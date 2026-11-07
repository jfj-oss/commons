use crate::envelope::*;
use crate::mls::{self, Prov};
use crate::state::*;
use crate::transport::*;
use crate::vault::{KdfParams, Vault};
use anyhow::{anyhow, bail, Context, Result};
use commons_proto::*;
use ed25519_dalek::SigningKey;
use rand::RngCore;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
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
    /// Exclusive lock on the data folder, held for as long as this client lives.
    lock: Option<std::fs::File>,
    /// How long a looked-up device list stays good, and how often an admin compares the relay's ACL with the group.
    pub dir_ttl_secs: u64,
    pub acl_check_secs: u64,
    /// Device lists and key packages are fetched between rounds, so that planning a change needs no network.
    pub(crate) dir_cache: HashMap<String, (u64, Vec<(DeviceCert, bool)>)>,
    pub(crate) dir_wanted: BTreeSet<String>,
    pub(crate) kp_cache: HashMap<String, Vec<Vec<u8>>>,
    pub(crate) kp_wanted: BTreeMap<String, u32>,
    pub(crate) kp_refill: bool,
    pub(crate) seen_welcomes: HashSet<String>,
    pub(crate) last_parked_try: u64,
    /// Another round would do something useful right now: more entries are waiting, or a lookup just came back.
    pub more: bool,
    pub(crate) sync_cap: u64,
    pub(crate) pair_ack: Option<(String, String)>,
    pub net: NetStatus,
}

/// How the connection to the relay looks from here, for the UI.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum NetState { #[default] Connecting, Online, Offline }
#[derive(Debug, Clone, Default)]
pub struct NetStatus { pub state: NetState, pub detail: String }

#[derive(Debug, Clone)]
pub struct ChannelInfo { pub gid: String, pub name: String, pub joined: bool, pub unread: usize }
#[derive(Debug, Clone)]
pub struct MemberInfo { pub account: String, pub name: String, pub role: Role, pub devices: usize, pub verified: bool, pub is_me: bool, pub tag: String, pub same_name: bool }

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
    // 35 bytes are exactly 56 base32 characters; a character more or less is a typo, not something to ignore.
    if d.len() < 35 { bail!("recovery key is too short"); }
    if s.chars().filter(|c| !c.is_whitespace() && *c != '-').count() != 56 { bail!("recovery key is too long"); }
    let sec: [u8; 32] = d[..32].try_into().unwrap();
    if d[32..35] != sha256(&[b"commons/recovery", &sec])[..3] { bail!("recovery key checksum does not match (typo?)"); }
    Ok(sec)
}

impl Client {
    // ------------------------------------------------------------------ lifecycle
    fn state_path(dir: &Path) -> PathBuf { dir.join("state.cmns") }
    pub fn exists(dir: &Path) -> bool { Vault::exists(&Self::state_path(dir)) }

    /// One Client per data folder, ever: two processes sharing state would both hand out the same key material.
    /// The kernel drops the lock when the process dies, so a crash never leaves the folder stuck.
    fn lock_dir(dir: &Path) -> Result<std::fs::File> {
        std::fs::create_dir_all(dir)?;
        let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("lock"))?;
        match f.try_lock() {
            Ok(()) => Ok(f),
            Err(std::fs::TryLockError::WouldBlock) => bail!("this account is already open in another Commons window or process; close that one first"),
            Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
        }
    }
    fn assemble(dir: &Path, vault: Vault, st: State, prov: Prov, t: Arc<dyn Transport>, lock: std::fs::File) -> Client {
        Client { dir: dir.into(), vault, st, prov, api: Api { t }, synced: BTreeSet::new(), typing: BTreeMap::new(), applied: BTreeSet::new(), acl_delay_secs: 120, last_saved: [0; 32], activity: 0, last_dir_check: HashMap::new(),
            lock: Some(lock), dir_ttl_secs: 30, acl_check_secs: 60, dir_cache: HashMap::new(), dir_wanted: BTreeSet::new(), kp_cache: HashMap::new(), kp_wanted: BTreeMap::new(), kp_refill: false, seen_welcomes: HashSet::new(), last_parked_try: 0,
            more: false, sync_cap: SYNC_MAX_BYTES, pair_ack: None, net: NetStatus::default() }
    }
    fn blank(dir: &Path, vault: Vault, relay: &str, name: &str, t: Arc<dyn Transport>, lock: std::fs::File) -> Client {
        let mut st = State::default();
        st.v = 1; st.relay = relay.into(); st.name = name.into();
        Self::assemble(dir, vault, st, Prov::default(), t, lock)
    }

    /// First device of a new account. Generates account + device keys on this machine; nothing is sent
    /// anywhere except the public cert (registered with the relay).
    pub fn create(dir: &Path, pass: &str, relay: &str, name: &str, kdf: KdfParams, t: Arc<dyn Transport>) -> Result<Client> {
        Self::create_with_invite(dir, pass, relay, name, "", kdf, t)
    }
    /// Same, for a relay that needs an invite secret from new accounts.
    pub fn create_with_invite(dir: &Path, pass: &str, relay: &str, name: &str, invite: &str, kdf: KdfParams, t: Arc<dyn Transport>) -> Result<Client> {
        let lock = Self::lock_dir(dir)?;
        let vault = Vault::create(&Self::state_path(dir), pass, kdf)?;
        let mut c = Self::blank(dir, vault, relay, name, t, lock);
        c.st.relay_invite = invite.into();
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
        let lock = Self::lock_dir(dir)?;
        let vault = Vault::create(&Self::state_path(dir), pass, kdf)?;
        let mut c = Self::blank(dir, vault, relay, name, t, lock);
        c.install_identity(&SigningKey::from_bytes(&secret))?;
        c.save()?;
        let _ = c.ensure_registered();
        c.save()?;
        Ok(c)
    }
    /// A device that has no identity yet and waits to be paired from an existing device (QR / code).
    pub fn create_for_pairing(dir: &Path, pass: &str, relay: &str, name: &str, kdf: KdfParams, t: Arc<dyn Transport>) -> Result<(Client, String)> {
        let lock = Self::lock_dir(dir)?;
        let vault = Vault::create(&Self::state_path(dir), pass, kdf)?;
        let mut c = Self::blank(dir, vault, relay, name, t, lock);
        let sk = crypto_box::SecretKey::generate(&mut rand::rngs::OsRng);
        let rid = rand_hex(16);
        c.st.pairing = Some(Pairing { rid, secret: hex::encode(sk.to_bytes()), public: hex::encode(sk.public_key().as_bytes()) });
        c.save()?;
        let code = c.pairing_code().ok_or_else(|| anyhow!("no pairing in progress"))?;
        Ok((c, code))
    }
    /// The code for the pairing in progress, shown again after an app restart.
    pub fn pairing_code(&self) -> Option<String> {
        let p = self.st.pairing.as_ref()?;
        let code = serde_json::json!({"relay": self.st.relay, "rid": p.rid, "pk": p.public}).to_string();
        Some(format!("commons-pair:{}", b64(code.as_bytes())))
    }
    /// Give up on a pairing that never finished. This device holds no identity yet, so nothing of value is lost.
    pub fn abandon_pairing(self) -> Result<()> {
        if self.has_identity() { bail!("this device already has an account"); }
        Vault::remove(&Self::state_path(&self.dir))
    }
    /// Like `open`, but the transport is built from the relay URL stored inside the encrypted state.
    pub fn open_with(dir: &Path, pass: &str, mk: impl FnOnce(&str) -> Arc<dyn Transport>) -> Result<Client> {
        let lock = Self::lock_dir(dir)?;
        let (vault, bytes) = Vault::open(&Self::state_path(dir), pass)?;
        let st: State = serde_json::from_slice(&bytes).context("state file parse")?;
        let prov = if st.mls.is_empty() { Prov::default() } else { Prov::load(&unb64(&st.mls)?)? };
        let t = mk(&st.relay);
        let recovered = vault.recovered;
        let mut c = Self::assemble(dir, vault, st, prov, t, lock);
        if recovered {
            c.notice("state", "The newest copy of this device's saved state was damaged, so the previous one was loaded. The last few moments may be missing; if this device sent anything in that time, it will say so and refresh its keys.");
            c.save()?;
        }
        Ok(c)
    }
    pub fn open(dir: &Path, pass: &str, t: Arc<dyn Transport>) -> Result<Client> { Self::open_with(dir, pass, |_| t) }

    /// Point this account at a relay's new address. For a relay that moved with its data, not for switching to a
    /// different relay: groups live on the relay, so a different one starts empty.
    pub fn set_relay(&mut self, relay: &str, t: Arc<dyn Transport>) -> Result<()> {
        self.st.relay = relay.into();
        self.api = Api { t };
        self.net = NetStatus::default();
        self.notice("relay", format!("relay address changed to {relay}"));
        self.save()
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

    #[doc(hidden)]
    pub fn debug_release_lock(&mut self) { self.lock = None; }

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
    /// For network work done outside the client's lock.
    pub fn net_handle(&self) -> Result<(Api, SigningKey)> { Ok((self.api.clone(), self.dev_key()?)) }

    pub fn notice(&mut self, kind: &str, text: impl Into<String>) {
        let text = text.into();
        if self.st.notices.back().is_some_and(|n| n.kind == kind && n.text == text) { return; } // no log spam from retried failures
        log::info!("[{}] {}", kind, text);
        self.st.notices.push_back(Notice { ts: now_secs(), kind: kind.into(), text });
        while self.st.notices.len() > 300 { self.st.notices.pop_front(); }
        self.activity += 1;
    }

    /// The recovery key is the whole identity: showing it needs the passphrase again, so an unlocked, unattended window gives it up less easily.
    pub fn recovery_key_with_passphrase(&self, pass: &str) -> Result<String> {
        if !Vault::check_passphrase(&Self::state_path(&self.dir), pass) { bail!("wrong passphrase"); }
        self.recovery_key()
    }
    pub fn recovery_key(&self) -> Result<String> { Ok(encode_recovery(&self.acct_key()?.to_bytes())) }

    // ------------------------------------------------------------------ registration and key packages
    fn invite(&self) -> Option<String> { Some(self.st.relay_invite.clone()).filter(|s| !s.is_empty()) }
    pub(crate) fn register_invite(&self) -> Option<String> { self.invite() }
    /// The relay has no record of us: its data is gone, or it is a different relay.
    pub(crate) fn relay_forgot_us(&mut self) {
        self.st.registered = false;
        self.st.lr_published = false;
        self.last_dir_check.remove("kp");
        self.notice("relay", "The relay no longer knows this device. It may have lost its data, so messages it was holding may be gone and communities may need a new invite. Registering again.");
    }
    pub fn ensure_registered(&mut self) -> Result<()> {
        let k = self.dev_key()?;
        if !self.st.registered {
            self.api.register(&k, &self.cert()?, self.invite())?;
            self.st.registered = true;
        }
        let have = match self.api.kp_count(&k, &self.my_device()) {
            Err(e) if relay_says(&e, 403, "not registered") => {
                self.relay_forgot_us();
                self.api.register(&k, &self.cert()?, self.invite())?;
                self.st.registered = true;
                0
            }
            r => r?,
        };
        if have < 10 {
            let (pk, lr) = self.build_key_packages()?;
            self.api.publish_kps(&k, pk, lr)?;
            self.st.lr_published = true;
        }
        Ok(())
    }
    /// Private halves go into the MLS store and are saved before the public halves leave.
    pub(crate) fn build_key_packages(&mut self) -> Result<(Vec<Vec<u8>>, Option<Vec<u8>>)> {
        let (signer, cert) = (self.signer()?, self.cert()?);
        let mut pk = vec![];
        for _ in 0..20 { pk.push(mls::build_kp(&self.prov, &signer, &cert, false)?); }
        let lr = if !self.st.lr_published { Some(mls::build_kp(&self.prov, &signer, &cert, true)?) } else { None };
        self.save()?;
        Ok((pk, lr))
    }

    // ------------------------------------------------------------------ pairing (second device)
    /// On the NEW device: check whether the existing device has sent us the account key.
    pub fn pairing_poll(&mut self) -> Result<bool> {
        let Some(p) = self.st.pairing.clone() else { return Ok(self.has_identity()) };
        let Some(sealed) = self.api.rv_get(&p.rid, "a")? else { return Ok(false) };
        if !self.pairing_apply(&sealed)? { return Ok(false); }
        self.ensure_registered()?;
        self.save()?;
        if let Some((rid, dev)) = self.pair_ack.take() { self.api.rv_put(&self.dev_key()?, &rid, "b", dev.as_bytes())?; }
        Ok(true)
    }
    /// Something arrived at the rendezvous. Whoever can reach that mailbox can leave anything in it, so what does not
    /// open with our one-time key is ignored: the wait goes on, the pairing is not lost.
    pub(crate) fn pairing_apply(&mut self, sealed: &[u8]) -> Result<bool> {
        let Some(p) = self.st.pairing.clone() else { return Ok(self.has_identity()) };
        let sk = crypto_box::SecretKey::from_bytes(hex32(&p.secret)?.into());
        #[derive(serde::Deserialize)] struct P { account_secret: String, name: String }
        let Some(pl) = sk.unseal(sealed).ok().and_then(|plain| serde_json::from_slice::<P>(&plain).ok()) else {
            self.notice("pair", "ignored something at the pairing address that was not sealed for this device");
            return Ok(false);
        };
        let acct = SigningKey::from_bytes(&hex32(&pl.account_secret)?);
        self.install_identity(&acct)?;
        if self.st.name.is_empty() { self.st.name = if check_name("name", pl.name.trim()).is_ok() { pl.name.trim().into() } else { "me".into() }; }
        self.st.pairing = None;
        self.st.registered = false;
        self.save()?;
        self.pair_ack = Some((p.rid, self.my_device()));
        self.notice("pair", "this device is now paired; the other device will add it to your communities");
        Ok(true)
    }
    /// The check digits for a code, before anything is sent: the person compares them first.
    pub fn pairing_check_code(&self, code: &str) -> Result<String> {
        let (_, pk) = self.parse_pair_code(code)?;
        Ok(pairing_check(&pk))
    }
    fn parse_pair_code(&self, code: &str) -> Result<(String, String)> {
        let raw = code.trim().strip_prefix("commons-pair:").ok_or_else(|| anyhow!("not a pairing code"))?;
        let j: serde_json::Value = unb64(raw).ok().and_then(|b| serde_json::from_slice(&b).ok()).ok_or_else(|| anyhow!("bad pairing code"))?;
        let (rid, pk) = (j["rid"].as_str().unwrap_or(""), j["pk"].as_str().unwrap_or(""));
        if !is_id(rid) || hex32(pk).is_err() { bail!("bad pairing code"); }
        if j["relay"].as_str().map(|r| r.trim_end_matches('/')) != Some(self.st.relay.trim_end_matches('/')) { bail!("the new device uses a different relay"); }
        Ok((rid.into(), pk.into()))
    }
    /// On the EXISTING device: seal the account key to the one-time key in the code shown on the new device.
    /// Returns the mailbox, the sealed bytes and the check digits.
    pub fn pairing_seal(&self, code: &str) -> Result<(String, Vec<u8>, String)> {
        let (rid, pk) = self.parse_pair_code(code)?;
        let pkb = crypto_box::PublicKey::from(hex32(&pk)?);
        let payload = serde_json::to_vec(&serde_json::json!({"account_secret": self.st.account_secret, "name": self.st.name}))?;
        let sealed = pkb.seal(&mut rand::rngs::OsRng, &payload).map_err(|_| anyhow!("seal failed"))?;
        Ok((rid, sealed, pairing_check(&pk)))
    }
    pub fn pairing_approve(&mut self, code: &str) -> Result<String> {
        let (rid, sealed, check) = self.pairing_seal(code)?;
        self.api.rv_put(&self.dev_key()?, &rid, "a", &sealed)?;
        self.notice("pair", "pairing data sent; the new device will appear in your device list once it registers");
        Ok(check)
    }
    pub fn pairing_check_new(&self) -> Option<String> {
        let p = self.st.pairing.as_ref()?;
        Some(pairing_check(&p.public))
    }

    // ------------------------------------------------------------------ directory
    pub fn directory(&mut self, account: &str) -> Result<Vec<(DeviceCert, bool)>> {
        let k = self.dev_key()?;
        let list = self.api.devices(&k, account)?;
        Ok(self.merge_directory(account, list))
    }
    /// A hostile relay could lie about a device list, so certs are checked here, and revocations remembered.
    pub fn merge_directory(&mut self, account: &str, list: Vec<DeviceInfo>) -> Vec<(DeviceCert, bool)> {
        let mut out = vec![];
        for d in list {
            if d.cert.account != account || d.cert.verify().is_err() { continue; }
            let revoked = match &d.revoked_sig {
                Some(s) if verify_revocation(account, &d.cert.device, s).is_ok() => { self.st.revoked.insert(d.cert.device.clone(), s.clone()); true }
                _ => self.st.revoked.contains_key(&d.cert.device),
            };
            out.push((d.cert, revoked));
        }
        if account == self.my_account() { self.watch_own_devices(&out); }
        out
    }
    /// A stolen device still holds the account key and can enroll another device for this account (see
    /// THREAT-MODEL.md); the relay cannot tell. What we can do is make sure a device nobody expected gets noticed.
    fn watch_own_devices(&mut self, list: &[(DeviceCert, bool)]) {
        let first = self.st.known_devices.is_empty();
        let me = self.my_device();
        let mut fresh = vec![];
        for (c, revoked) in list {
            if self.st.known_devices.insert(c.device.clone()) && !*revoked && c.device != me { fresh.push(c.device.clone()); }
        }
        if first { return; }
        for d in fresh {
            self.notice("device", format!("A new device ({}...) was added to your account. If that was not you, revoke it now. A revoked device that was stolen can do this again, because it still holds your account key: then the safe way out is a new account.", &d[..8]));
        }
    }
    pub fn my_devices(&mut self) -> Result<Vec<(DeviceCert, bool)>> { let a = self.my_account(); self.directory(&a) }

    /// Revoke a lost device. Needs the account key (any of your devices, or one made from the recovery key).
    pub fn revoke_device(&mut self, device: &str) -> Result<()> {
        let sig = self.revocation_sig(device)?;
        self.api.revoke(&self.my_account(), device, &sig)?;
        self.revocation_done(device, sig)
    }
    pub fn revocation_sig(&self, device: &str) -> Result<String> { Ok(sign_revocation(&self.acct_key()?, &hex32(device)?)) }
    /// The relay has it: tell every community too, in case the relay hides it from someone.
    pub fn revocation_done(&mut self, device: &str, sig: String) -> Result<()> {
        self.st.revoked.insert(device.to_string(), sig.clone());
        let env = Envelope::Revoked { account: self.my_account(), device: device.into(), sig };
        let ctls: Vec<String> = self.st.groups.values().filter(|g| g.kind == Kind::Control && !g.removed).map(|g| g.gid.clone()).collect();
        for gid in ctls { self.queue_env(&gid, env.clone()); }
        self.notice("device", format!("revoked device {}. It can no longer read new messages. If it was stolen it still holds your account key, so watch this list: it could enroll a new device, and revoking only stops that one too.", &device[..8.min(device.len())]));
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
