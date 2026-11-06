use crate::envelope::*;
use crate::mls::{self, Prov};
use crate::state::*;
use crate::transport::*;
use crate::vault::{KdfParams, Vault};
use anyhow::{anyhow, bail, Context, Result};
use commons_proto::*;
use ed25519_dalek::SigningKey;
use rand::RngCore;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn rand_hex(n: usize) -> String { let mut b = vec![0u8; n]; rand::rngs::OsRng.fill_bytes(&mut b); hex::encode(b) }
pub(crate) fn hex32(s: &str) -> Result<[u8; 32]> { Ok(unhex32(s)?) }

pub struct Client {
    pub dir: PathBuf,
    pub(crate) vault: Vault,
    pub st: State,
    pub(crate) prov: Prov,
    pub(crate) api: Api,
    /// ids of groups we have fully caught up on since this process started (safe to encrypt)
    pub(crate) synced: BTreeSet<String>,
    pub typing: BTreeMap<String, u64>,
    pub(crate) applied: BTreeSet<String>,
    /// How long a removed member keeps relay read access so that it can still fetch (and understand) its own removal.
    pub acl_delay_secs: u64,
    pub(crate) last_saved: [u8; 32],
    /// number of "things happened" since start; the harness uses it to detect quiescence
    pub activity: u64,
    pub(crate) last_dir_check: HashMap<String, u64>,
}

#[derive(Debug, Clone)]
pub struct ChannelInfo { pub gid: String, pub name: String, pub joined: bool, pub unread: usize }
#[derive(Debug, Clone)]
pub struct MemberInfo { pub account: String, pub name: String, pub role: Role, pub devices: usize, pub verified: bool, pub is_me: bool }

fn base32_enc(data: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let (mut out, mut buf, mut bits) = (String::new(), 0u32, 0u32);
    for &b in data { buf = (buf << 8) | b as u32; bits += 8; while bits >= 5 { out.push(A[((buf >> (bits - 5)) & 31) as usize] as char); bits -= 5; } }
    if bits > 0 { out.push(A[((buf << (5 - bits)) & 31) as usize] as char); }
    out
}
fn base32_dec(s: &str) -> Option<Vec<u8>> {
    let (mut out, mut buf, mut bits) = (vec![], 0u32, 0u32);
    for c in s.chars().filter(|c| !c.is_whitespace() && *c != '-') {
        let v = match c.to_ascii_uppercase() { 'A'..='Z' => c.to_ascii_uppercase() as u32 - 'A' as u32, '2'..='7' => c as u32 - '2' as u32 + 26, _ => return None };
        buf = (buf << 5) | v; bits += 5;
        if bits >= 8 { out.push(((buf >> (bits - 8)) & 255) as u8); bits -= 8; }
    }
    Some(out)
}
pub fn encode_recovery(secret: &[u8; 32]) -> String {
    let mut d = secret.to_vec(); d.extend_from_slice(&sha256(&[b"commons/recovery", secret])[..3]);
    let s = base32_enc(&d);
    s.as_bytes().chunks(5).map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join("-")
}
pub fn decode_recovery(s: &str) -> Result<[u8; 32]> {
    let d = base32_dec(s).ok_or_else(|| anyhow!("recovery key has invalid characters"))?;
    if d.len() < 35 { bail!("recovery key is too short"); }
    let sec: [u8; 32] = d[..32].try_into().unwrap();
    if d[32..35] != sha256(&[b"commons/recovery", &sec])[..3] { bail!("recovery key checksum does not match (typo?)"); }
    Ok(sec)
}

impl Client {
    // ------------------------------------------------------------------ lifecycle
    fn state_path(dir: &Path) -> PathBuf { dir.join("state.cmns") }
    pub fn exists(dir: &Path) -> bool { Vault::exists(&Self::state_path(dir)) }

    fn blank(dir: &Path, vault: Vault, relay: &str, name: &str, t: Arc<dyn Transport>) -> Client {
        let mut st = State::default();
        st.v = 1; st.relay = relay.into(); st.name = name.into();
        Client { dir: dir.into(), vault, st, prov: Prov::default(), api: Api { t }, synced: BTreeSet::new(), typing: BTreeMap::new(), applied: BTreeSet::new(), acl_delay_secs: 120, last_saved: [0; 32], activity: 0, last_dir_check: HashMap::new() }
    }

    /// First device of a new account. Generates account + device keys on this machine; nothing is sent
    /// anywhere except the public cert (registered with the relay).
    pub fn create(dir: &Path, pass: &str, relay: &str, name: &str, kdf: KdfParams, t: Arc<dyn Transport>) -> Result<Client> {
        let vault = Vault::create(&Self::state_path(dir), pass, kdf)?;
        let mut c = Self::blank(dir, vault, relay, name, t);
        let acct = SigningKey::generate(&mut rand::rngs::OsRng);
        c.install_identity(&acct)?;
        c.save()?;
        let _ = c.ensure_registered();
        c.save()?;
        Ok(c)
    }
    /// Fresh device for an existing account, from its recovery key. Old messages are not recovered (see DESIGN.md).
    pub fn create_from_recovery(dir: &Path, pass: &str, relay: &str, name: &str, recovery: &str, kdf: KdfParams, t: Arc<dyn Transport>) -> Result<Client> {
        let secret = decode_recovery(recovery)?;
        let vault = Vault::create(&Self::state_path(dir), pass, kdf)?;
        let mut c = Self::blank(dir, vault, relay, name, t);
        c.install_identity(&SigningKey::from_bytes(&secret))?;
        c.save()?;
        let _ = c.ensure_registered();
        c.save()?;
        Ok(c)
    }
    /// A device that has no identity yet and waits to be paired from an existing device (QR / code).
    pub fn create_for_pairing(dir: &Path, pass: &str, relay: &str, name: &str, kdf: KdfParams, t: Arc<dyn Transport>) -> Result<(Client, String)> {
        let vault = Vault::create(&Self::state_path(dir), pass, kdf)?;
        let mut c = Self::blank(dir, vault, relay, name, t);
        let sk = crypto_box::SecretKey::generate(&mut rand::rngs::OsRng);
        let rid = rand_hex(16);
        let code = serde_json::to_string(&serde_json::json!({"relay": relay, "rid": rid, "pk": hex::encode(sk.public_key().as_bytes())}))?;
        c.st.pairing = Some(Pairing { rid, secret: hex::encode(sk.to_bytes()), public: hex::encode(sk.public_key().as_bytes()) });
        c.save()?;
        Ok((c, format!("commons-pair:{}", b64(code.as_bytes()))))
    }
    /// Like `open`, but the transport is built from the relay URL stored inside the encrypted state.
    pub fn open_with(dir: &Path, pass: &str, mk: impl FnOnce(&str) -> Arc<dyn Transport>) -> Result<Client> {
        let (vault, bytes) = Vault::open(&Self::state_path(dir), pass)?;
        let st: State = serde_json::from_slice(&bytes).context("state file parse")?;
        let prov = if st.mls.is_empty() { Prov::default() } else { Prov::load(&unb64(&st.mls)?)? };
        let t = mk(&st.relay);
        Ok(Client { dir: dir.into(), vault, st, prov, api: Api { t }, synced: BTreeSet::new(), typing: BTreeMap::new(), applied: BTreeSet::new(), acl_delay_secs: 120, last_saved: [0; 32], activity: 0, last_dir_check: HashMap::new() })
    }
    pub fn open(dir: &Path, pass: &str, t: Arc<dyn Transport>) -> Result<Client> {
        let (vault, bytes) = Vault::open(&Self::state_path(dir), pass)?;
        let st: State = serde_json::from_slice(&bytes).context("state file parse")?;
        let prov = if st.mls.is_empty() { Prov::default() } else { Prov::load(&unb64(&st.mls)?)? };
        Ok(Client { dir: dir.into(), vault, st, prov, api: Api { t }, synced: BTreeSet::new(), typing: BTreeMap::new(), applied: BTreeSet::new(), acl_delay_secs: 120, last_saved: [0; 32], activity: 0, last_dir_check: HashMap::new() })
    }

    fn install_identity(&mut self, acct: &SigningKey) -> Result<()> {
        let dev = SigningKey::generate(&mut rand::rngs::OsRng);
        let cert = DeviceCert::issue(acct, &dev.verifying_key().to_bytes());
        self.st.account_secret = hex::encode(acct.to_bytes());
        self.st.device_secret = hex::encode(dev.to_bytes());
        self.st.cert = Some(cert);
        Ok(())
    }

    pub fn save(&mut self) -> Result<()> {
        self.st.mls = b64(&self.prov.dump());
        let bytes = serde_json::to_vec(&self.st)?;
        let h = sha256(&[&bytes]);
        if h == self.last_saved { return Ok(()); } // nothing changed: no disk writes while idle
        self.vault.save(&bytes)?;
        self.last_saved = h;
        Ok(())
    }

    pub fn has_identity(&self) -> bool { self.st.cert.is_some() }
    pub fn my_account(&self) -> String { self.st.cert.as_ref().map(|c| c.account.clone()).unwrap_or_default() }
    pub fn my_device(&self) -> String { self.st.cert.as_ref().map(|c| c.device.clone()).unwrap_or_default() }
    pub(crate) fn dev_key(&self) -> Result<SigningKey> { Ok(SigningKey::from_bytes(&hex32(&self.st.device_secret).map_err(|_| anyhow!("no identity yet"))?)) }
    pub(crate) fn acct_key(&self) -> Result<SigningKey> { Ok(SigningKey::from_bytes(&hex32(&self.st.account_secret).map_err(|_| anyhow!("no identity yet"))?)) }
    pub(crate) fn signer(&self) -> Result<openmls_basic_credential::SignatureKeyPair> {
        let k = self.dev_key()?;
        Ok(mls::signer(&k.to_bytes(), &k.verifying_key().to_bytes()))
    }
    pub(crate) fn cert(&self) -> Result<DeviceCert> { self.st.cert.clone().ok_or_else(|| anyhow!("no identity yet")) }

    pub fn notice(&mut self, kind: &str, text: impl Into<String>) {
        let text = text.into();
        if self.st.notices.back().is_some_and(|n| n.kind == kind && n.text == text) { return; } // no log spam from retried failures
        log::info!("[{}] {}", kind, text);
        self.st.notices.push_back(Notice { ts: now_secs(), kind: kind.into(), text });
        while self.st.notices.len() > 300 { self.st.notices.pop_front(); }
        self.activity += 1;
    }

    pub fn recovery_key(&self) -> Result<String> { Ok(encode_recovery(&self.acct_key()?.to_bytes())) }

    // ------------------------------------------------------------------ registration and key packages
    pub fn ensure_registered(&mut self) -> Result<()> {
        let k = self.dev_key()?;
        if !self.st.registered {
            self.api.register(&k, &self.cert()?)?;
            self.st.registered = true;
        }
        let have = self.api.kp_count(&k, &self.my_device())?;
        if have < 10 {
            let (signer, cert) = (self.signer()?, self.cert()?);
            let mut pk = vec![];
            for _ in 0..20 { pk.push(mls::build_kp(&self.prov, &signer, &cert, false)?); }
            let lr = if !self.st.lr_published { Some(mls::build_kp(&self.prov, &signer, &cert, true)?) } else { None };
            self.save()?; // private halves are in the MLS store: persist before publishing
            self.api.publish_kps(&k, pk, lr)?;
            self.st.lr_published = true;
        }
        Ok(())
    }

    // ------------------------------------------------------------------ pairing (second device)
    /// On the NEW device: check whether the existing device has sent us the account key.
    pub fn pairing_poll(&mut self) -> Result<bool> {
        let Some(p) = self.st.pairing.clone() else { return Ok(self.has_identity()) };
        let Some(sealed) = self.api.rv_get(&p.rid, "a")? else { return Ok(false) };
        let sk = crypto_box::SecretKey::from_bytes(hex32(&p.secret)?.into());
        let plain = sk.unseal(&sealed).map_err(|_| anyhow!("pairing payload could not be opened (wrong QR or tampered)"))?;
        #[derive(serde::Deserialize)] struct P { account_secret: String, name: String }
        let pl: P = serde_json::from_slice(&plain).context("pairing payload")?;
        let acct = SigningKey::from_bytes(&hex32(&pl.account_secret)?);
        self.install_identity(&acct)?;
        if self.st.name.is_empty() { self.st.name = pl.name; }
        self.st.pairing = None;
        self.save()?;
        self.ensure_registered()?;
        self.save()?;
        let cert = self.cert()?;
        self.api.rv_put(&p.rid, "b", cert.device.as_bytes())?;
        self.notice("pair", "this device is now paired; the other device will add it to your communities");
        Ok(true)
    }
    /// On the EXISTING device: given the code shown (or scanned) on the new device, send the account key sealed to it.
    /// Returns the check string both screens should show.
    pub fn pairing_approve(&mut self, code: &str) -> Result<String> {
        let raw = code.trim().strip_prefix("commons-pair:").ok_or_else(|| anyhow!("not a pairing code"))?;
        let j: serde_json::Value = serde_json::from_slice(&unb64(raw)?)?;
        let (rid, pk) = (j["rid"].as_str().unwrap_or(""), j["pk"].as_str().unwrap_or(""));
        if rid.len() != 32 || pk.len() != 64 { bail!("bad pairing code"); }
        if j["relay"].as_str() != Some(self.st.relay.as_str()) { bail!("the new device uses a different relay"); }
        let pkb = crypto_box::PublicKey::from(hex32(pk)?);
        let payload = serde_json::to_vec(&serde_json::json!({"account_secret": self.st.account_secret, "name": self.st.name}))?;
        let sealed = pkb.seal(&mut rand::rngs::OsRng, &payload).map_err(|_| anyhow!("seal failed"))?;
        self.api.rv_put(rid, "a", &sealed)?;
        self.notice("pair", "pairing data sent; the new device will appear in your device list once it registers");
        Ok(pairing_check(pk))
    }
    pub fn pairing_check_new(&self) -> Option<String> {
        let p = self.st.pairing.as_ref()?;
        Some(pairing_check(&p.public))
    }

    // ------------------------------------------------------------------ directory
    pub fn directory(&mut self, account: &str) -> Result<Vec<(DeviceCert, bool)>> {
        let k = self.dev_key()?;
        let list = self.api.devices(&k, account)?;
        let mut out = vec![];
        for d in list {
            if d.cert.account != account || d.cert.verify().is_err() { continue; } // hostile relay could lie; certs are self-verifying
            let revoked = match &d.revoked_sig {
                Some(s) if verify_revocation(account, &d.cert.device, s).is_ok() => { self.st.revoked.insert(d.cert.device.clone(), s.clone()); true }
                _ => self.st.revoked.contains_key(&d.cert.device),
            };
            out.push((d.cert, revoked));
        }
        Ok(out)
    }
    pub fn my_devices(&mut self) -> Result<Vec<(DeviceCert, bool)>> { let a = self.my_account(); self.directory(&a) }

    /// Revoke a lost device. Needs the account key (any of your devices, or one made from the recovery key).
    pub fn revoke_device(&mut self, device: &str) -> Result<()> {
        let ak = self.acct_key()?;
        let d = hex32(device)?;
        let sig = sign_revocation(&ak, &d);
        self.api.revoke(&self.my_account(), device, &sig)?;
        self.st.revoked.insert(device.to_string(), sig.clone());
        let env = Envelope::Revoked { account: self.my_account(), device: device.into(), sig };
        let ctls: Vec<String> = self.st.groups.values().filter(|g| g.kind == Kind::Control && !g.removed).map(|g| g.gid.clone()).collect();
        for gid in ctls { self.queue_env(&gid, env.clone()); }
        self.notice("device", format!("revoked device {}", &device[..8.min(device.len())]));
        self.save()
    }
}

/// Four digits both screens show, derived from the new device's one-time key, so a person can confirm that
/// the code they approve is the one on the screen in front of them.
pub fn pairing_check(new_pk: &str) -> String {
    let h = sha256(&[b"commons/pair-check", new_pk.as_bytes()]);
    format!("{:04}", u32::from_be_bytes(h[..4].try_into().unwrap()) % 10000)
}

pub fn safety_number(a: &str, b: &str) -> String {
    let (x, y) = if a <= b { (a, b) } else { (b, a) };
    let mut h = sha256(&[b"commons/safety/v1", x.as_bytes(), y.as_bytes()]);
    for _ in 0..5000 { h = sha256(&[b"commons/safety/iter", &h, x.as_bytes(), y.as_bytes()]); }
    let mut ext = h.to_vec(); ext.extend_from_slice(&sha256(&[&h, b"x"])[..]);
    ext.chunks(5).take(12).map(|c| { let mut b = [0u8; 8]; b[3..].copy_from_slice(c); format!("{:05}", u64::from_be_bytes(b) % 100000) }).collect::<Vec<_>>().join(" ")
}
