//! Encrypted-at-rest storage for all client state (identity keys, MLS state, message history).
//! Passphrase -> Argon2id -> 32 byte key -> XChaCha20-Poly1305. No custom primitives.
//! Writes are atomic (temp file + fsync + rename) and the previous generation is kept as `.bak`.
use anyhow::{anyhow, bail, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{aead::{Aead, KeyInit, Payload}, XChaCha20Poly1305, XNonce};
use rand::RngCore;
use std::{fs, io::Write, path::{Path, PathBuf}};

const MAGIC: &[u8; 6] = b"CMNS\x01\x00";

#[derive(Clone, Copy, Debug)]
pub struct KdfParams { pub m_kib: u32, pub t: u32, pub p: u32 }
impl KdfParams {
    pub const STRONG: KdfParams = KdfParams { m_kib: 64 * 1024, t: 3, p: 1 };
    /// Only for tests: fast and weak.
    pub const TEST: KdfParams = KdfParams { m_kib: 8, t: 1, p: 1 };
}

pub struct Vault { path: PathBuf, key: [u8; 32], salt: [u8; 16], params: KdfParams }

fn derive(pass: &str, salt: &[u8; 16], p: KdfParams) -> Result<[u8; 32]> {
    let params = Params::new(p.m_kib, p.t, p.p, Some(32)).map_err(|e| anyhow!("kdf params: {e}"))?;
    let mut out = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(pass.as_bytes(), salt, &mut out).map_err(|e| anyhow!("kdf: {e}"))?;
    Ok(out)
}

fn header(p: KdfParams, salt: &[u8; 16]) -> Vec<u8> {
    let mut h = MAGIC.to_vec();
    h.extend_from_slice(&p.m_kib.to_be_bytes()); h.extend_from_slice(&p.t.to_be_bytes()); h.extend_from_slice(&p.p.to_be_bytes());
    h.extend_from_slice(salt);
    h
}

#[derive(Debug)]
pub enum VaultError { WrongPassphrase, Corrupt }
impl std::fmt::Display for VaultError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self { VaultError::WrongPassphrase => write!(f, "wrong passphrase"), VaultError::Corrupt => write!(f, "state file is corrupt") }
    }
}
impl std::error::Error for VaultError {}

fn try_open(path: &Path, pass: &str) -> Result<(Vault, Vec<u8>), anyhow::Error> {
    let raw = fs::read(path)?;
    if raw.len() < 6 + 12 + 16 + 24 + 16 || &raw[..6] != MAGIC { return Err(VaultError::Corrupt.into()); }
    let m = u32::from_be_bytes(raw[6..10].try_into().unwrap());
    let t = u32::from_be_bytes(raw[10..14].try_into().unwrap());
    let p = u32::from_be_bytes(raw[14..18].try_into().unwrap());
    if m > 1 << 21 || t > 20 || p > 8 || m < 8 { return Err(VaultError::Corrupt.into()); }
    let params = KdfParams { m_kib: m, t, p };
    let salt: [u8; 16] = raw[18..34].try_into().unwrap();
    let nonce = &raw[34..58];
    let key = derive(pass, &salt, params)?;
    let c = XChaCha20Poly1305::new((&key).into());
    let pt = c.decrypt(XNonce::from_slice(nonce), Payload { msg: &raw[58..], aad: &raw[..34] }).map_err(|_| VaultError::WrongPassphrase)?;
    Ok((Vault { path: path.to_path_buf(), key, salt, params }, pt))
}

impl Vault {
    pub fn exists(path: &Path) -> bool { path.exists() || path.with_extension("bak").exists() }
    pub fn create(path: &Path, pass: &str, params: KdfParams) -> Result<Vault> {
        if Vault::exists(path) { bail!("state already exists at {}", path.display()); }
        if let Some(d) = path.parent() { fs::create_dir_all(d)?; }
        let mut salt = [0u8; 16]; rand::rngs::OsRng.fill_bytes(&mut salt);
        Ok(Vault { path: path.to_path_buf(), key: derive(pass, &salt, params)?, salt, params })
    }
    /// Opens the newest readable generation. A wrong passphrase is reported as such, not as corruption.
    pub fn open(path: &Path, pass: &str) -> Result<(Vault, Vec<u8>)> {
        match try_open(path, pass) {
            Ok(r) => Ok(r),
            Err(e) => {
                if matches!(e.downcast_ref::<VaultError>(), Some(VaultError::WrongPassphrase)) { return Err(e); }
                let bak = path.with_extension("bak");
                if bak.exists() { if let Ok(r) = try_open(&bak, pass) { return Ok(r); } }
                Err(e)
            }
        }
    }
    pub fn key(&self) -> &[u8; 32] { &self.key }
    pub fn save(&self, plaintext: &[u8]) -> Result<()> {
        let mut nonce = [0u8; 24]; rand::rngs::OsRng.fill_bytes(&mut nonce);
        let h = header(self.params, &self.salt);
        let c = XChaCha20Poly1305::new((&self.key).into());
        let ct = c.encrypt(XNonce::from_slice(&nonce), Payload { msg: plaintext, aad: &h }).map_err(|_| anyhow!("encrypt"))?;
        let tmp = self.path.with_extension("tmp");
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&h)?; f.write_all(&nonce)?; f.write_all(&ct)?;
            f.sync_all()?;
        }
        if self.path.exists() { let _ = fs::rename(&self.path, self.path.with_extension("bak")); }
        fs::rename(&tmp, &self.path)?;
        if let Some(d) = self.path.parent() { if let Ok(dir) = fs::File::open(d) { let _ = dir.sync_all(); } }
        Ok(())
    }
    /// Encrypt/decrypt a side file (attachment cache) with the vault key.
    pub fn seal(&self, data: &[u8]) -> Result<Vec<u8>> {
        let mut nonce = [0u8; 24]; rand::rngs::OsRng.fill_bytes(&mut nonce);
        let c = XChaCha20Poly1305::new((&self.key).into());
        let mut out = nonce.to_vec();
        out.extend(c.encrypt(XNonce::from_slice(&nonce), data).map_err(|_| anyhow!("encrypt"))?);
        Ok(out)
    }
    pub fn unseal(&self, data: &[u8]) -> Result<Vec<u8>> {
        if data.len() < 24 + 16 { bail!("short"); }
        let c = XChaCha20Poly1305::new((&self.key).into());
        c.decrypt(XNonce::from_slice(&data[..24]), &data[24..]).map_err(|_| anyhow!("cache decrypt failed"))
    }
}
