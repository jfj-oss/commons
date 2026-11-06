//! Thin glue around OpenMLS (RFC 9420). Ciphersuite: MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519 (0x0001),
//! the RFC's mandatory-to-implement suite. All cryptography is inside openmls_rust_crypto / RustCrypto crates.
use crate::envelope::*;
use commons_proto::DeviceCert;
use openmls::prelude::*;
use openmls::prelude::tls_codec::{Deserialize as _, Serialize as _};
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::{MemoryStorage, RustCrypto};
use openmls_traits::{types::SignatureScheme, OpenMlsProvider};
use std::collections::HashMap;
use std::sync::RwLock;

pub const CS: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;
pub const MAX_PAST_EPOCHS: usize = 8;
const FIVE_YEARS: u64 = 5 * 365 * 24 * 3600;

#[derive(Default)]
pub struct Prov { crypto: RustCrypto, storage: MemoryStorage }
impl OpenMlsProvider for Prov {
    type CryptoProvider = RustCrypto;
    type RandProvider = RustCrypto;
    type StorageProvider = MemoryStorage;
    fn storage(&self) -> &Self::StorageProvider { &self.storage }
    fn crypto(&self) -> &Self::CryptoProvider { &self.crypto }
    fn rand(&self) -> &Self::RandProvider { &self.crypto }
}
impl Prov {
    pub fn dump(&self) -> Vec<u8> {
        let v = self.storage.values.read().unwrap();
        let mut out = (v.len() as u64).to_be_bytes().to_vec();
        for (k, val) in v.iter() {
            out.extend((k.len() as u64).to_be_bytes()); out.extend((val.len() as u64).to_be_bytes());
            out.extend(k); out.extend(val);
        }
        out
    }
    pub fn load(b: &[u8]) -> anyhow::Result<Prov> {
        let mut map = HashMap::new();
        let mut p = 0usize;
        let rd = |p: &mut usize, n: usize| -> anyhow::Result<&[u8]> {
            if *p + n > b.len() { anyhow::bail!("truncated mls state"); }
            let s = &b[*p..*p + n]; *p += n; Ok(s)
        };
        let count = u64::from_be_bytes(rd(&mut p, 8)?.try_into()?);
        for _ in 0..count {
            let kl = u64::from_be_bytes(rd(&mut p, 8)?.try_into()?) as usize;
            let vl = u64::from_be_bytes(rd(&mut p, 8)?.try_into()?) as usize;
            let k = rd(&mut p, kl)?.to_vec(); let v = rd(&mut p, vl)?.to_vec();
            map.insert(k, v);
        }
        let prov = Prov::default();
        *prov.storage.values.write().unwrap() = map;
        Ok(prov)
    }
}
#[allow(dead_code)]
fn _assert(_: &RwLock<()>) {}

pub fn signer(secret: &[u8; 32], public: &[u8; 32]) -> SignatureKeyPair {
    SignatureKeyPair::from_raw(SignatureScheme::ED25519, secret.to_vec(), public.to_vec())
}
pub fn cred_with_key(cert: &DeviceCert) -> anyhow::Result<CredentialWithKey> {
    let dev = commons_proto::unhex32(&cert.device)?;
    Ok(CredentialWithKey { credential: BasicCredential::new(cert.to_bytes()).into(), signature_key: dev.to_vec().into() })
}
pub fn caps() -> Capabilities {
    Capabilities::new(None, None, Some(&[ExtensionType::Unknown(EXT_COMMONS), ExtensionType::LastResort]), None, Some(&[CredentialType::Basic]))
}
pub fn ext_of(g: &GroupExt) -> anyhow::Result<Extensions<GroupContext>> {
    Ok(Extensions::from_vec(vec![
        Extension::RequiredCapabilities(RequiredCapabilitiesExtension::new(&[ExtensionType::Unknown(EXT_COMMONS)], &[], &[])),
        Extension::Unknown(EXT_COMMONS, UnknownExtension(g.to_bytes())),
    ])?)
}
pub fn group_ext(exts: &Extensions<GroupContext>) -> Option<GroupExt> {
    match exts.unknown(EXT_COMMONS) { Some(UnknownExtension(b)) => GroupExt::parse(b).ok(), None => None }
}
pub fn create_cfg(ext: &GroupExt) -> anyhow::Result<MlsGroupCreateConfig> {
    Ok(MlsGroupCreateConfig::builder()
        .ciphersuite(CS)
        .padding_size(256)
        .max_past_epochs(MAX_PAST_EPOCHS)
        .use_ratchet_tree_extension(true)
        .sender_ratchet_configuration(SenderRatchetConfiguration::new(64, 2000))
        .capabilities(caps())
        .lifetime(Lifetime::new(FIVE_YEARS))
        .with_group_context_extensions(ext_of(ext)?)
        .build())
}
pub fn join_cfg() -> MlsGroupJoinConfig {
    MlsGroupJoinConfig::builder()
        .padding_size(256)
        .max_past_epochs(MAX_PAST_EPOCHS)
        .use_ratchet_tree_extension(true)
        .sender_ratchet_configuration(SenderRatchetConfiguration::new(64, 2000))
        .build()
}
pub fn build_kp(prov: &Prov, signer: &SignatureKeyPair, cert: &DeviceCert, last_resort: bool) -> anyhow::Result<Vec<u8>> {
    let mut b = KeyPackage::builder().key_package_lifetime(Lifetime::new(FIVE_YEARS)).leaf_node_capabilities(caps());
    if last_resort { b = b.mark_as_last_resort(); }
    let bundle = b.build(CS, prov, signer, cred_with_key(cert)?)?;
    Ok(bundle.key_package().tls_serialize_detached()?)
}
/// Parse and validate a key package fetched from the (untrusted) directory.
/// Returns the package and the verified cert in its credential.
pub fn parse_kp(prov: &Prov, bytes: &[u8]) -> anyhow::Result<(KeyPackage, DeviceCert)> {
    let kpin = KeyPackageIn::tls_deserialize_exact(bytes)?;
    let kp = kpin.validate(prov.crypto(), ProtocolVersion::Mls10)?;
    let cert = cert_of(kp.leaf_node().credential(), kp.leaf_node().signature_key().as_slice())
        .ok_or_else(|| anyhow::anyhow!("key package credential is not a valid device cert"))?;
    Ok((kp, cert))
}
/// The credential identity must be a valid account-signed cert, and must name the leaf's signature key.
pub fn cert_of(c: &Credential, sig_key: &[u8]) -> Option<DeviceCert> {
    let cert = DeviceCert::from_bytes(c.serialized_content()).ok()?;
    if hex::decode(&cert.device).ok()? != sig_key { return None; }
    Some(cert)
}
pub fn members_of(g: &MlsGroup) -> Vec<(LeafNodeIndex, DeviceCert)> {
    g.members().filter_map(|m| cert_of(&m.credential, &m.signature_key).map(|c| (m.index, c))).collect()
}
