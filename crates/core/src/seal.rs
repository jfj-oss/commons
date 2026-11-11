//! Sealing bytes to the holder of one Ed25519 key (an account key, or a device key), so that the relay and every other member
//! can carry the bytes and read nothing. The recipient's Ed25519 public key is used as an X25519 key the way libsodium's
//! `crypto_sign_ed25519_pk_to_curve25519` does; a fresh ephemeral X25519 key agrees with it, and the plaintext rides under
//! ChaCha20-Poly1305 with the caller's associated data. Used for a direct call's invite secret (`direct.rs`, sealed to the
//! peer's account) and for history chunks to a new device (`history.rs`, sealed to that device's key).
//!
//! Layout: ephemeral public key (32) | nonce (12) | ciphertext.
use anyhow::{anyhow, Result};
use chacha20poly1305::{aead::{Aead, KeyInit, Payload}, ChaCha20Poly1305, Nonce};
use commons_proto::{sha256, unhex32};
use rand::RngCore;

/// The X25519 form of a hex Ed25519 public key.
pub fn x25519_of(ed_pk_hex: &str) -> Result<[u8; 32]> {
    let vk = ed25519_dalek::VerifyingKey::from_bytes(&unhex32(ed_pk_hex).map_err(|_| anyhow!("bad key"))?).map_err(|_| anyhow!("bad public key"))?;
    Ok(vk.to_montgomery().to_bytes())
}

fn key(eph_pk: &[u8; 32], recipient_x: &[u8; 32], shared: &[u8; 32]) -> [u8; 32] { sha256(&[b"commons/seal/v1", eph_pk, recipient_x, shared]) }

pub fn seal_to(ed_pk_hex: &str, aad: &[u8], plain: &[u8]) -> Result<Vec<u8>> {
    let recipient_x = x25519_of(ed_pk_hex)?;
    let mut eph = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut eph);
    let eph_pk = x25519_dalek::x25519(eph, x25519_dalek::X25519_BASEPOINT_BYTES);
    let shared = x25519_dalek::x25519(eph, recipient_x);
    let k = key(&eph_pk, &recipient_x, &shared);
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let ct = ChaCha20Poly1305::new((&k).into()).encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad }).map_err(|_| anyhow!("seal"))?;
    let mut out = eph_pk.to_vec(); out.extend(nonce); out.extend(ct);
    Ok(out)
}

/// Open with the Ed25519 secret key the bytes were sealed to. None when they were not, or were damaged.
pub fn open_with(sk: &ed25519_dalek::SigningKey, aad: &[u8], blob: &[u8]) -> Option<Vec<u8>> {
    if blob.len() < 32 + 12 + 16 { return None; }
    let eph_pk: [u8; 32] = blob[..32].try_into().ok()?;
    let my_x = sk.verifying_key().to_montgomery().to_bytes();
    let shared = x25519_dalek::x25519(sk.to_scalar_bytes(), eph_pk);
    let k = key(&eph_pk, &my_x, &shared);
    ChaCha20Poly1305::new((&k).into()).decrypt(Nonce::from_slice(&blob[32..44]), Payload { msg: &blob[44..], aad }).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_the_named_key_opens_and_the_aad_binds() {
        let (a, b) = (ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng), ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng));
        let pk = hex::encode(a.verifying_key().to_bytes());
        let blob = seal_to(&pk, b"ctx", b"hello").unwrap();
        assert_eq!(open_with(&a, b"ctx", &blob).as_deref(), Some(&b"hello"[..]));
        assert!(open_with(&b, b"ctx", &blob).is_none());
        assert!(open_with(&a, b"other", &blob).is_none());
        assert_ne!(seal_to(&pk, b"ctx", b"hello").unwrap(), blob);
        assert!(seal_to("zz", b"", b"").is_err());
        let mut t = blob.clone(); let n = t.len(); t[n - 1] ^= 1;
        assert!(open_with(&a, b"ctx", &t).is_none());
        assert!(open_with(&a, b"ctx", &blob[..40]).is_none());
    }
}
