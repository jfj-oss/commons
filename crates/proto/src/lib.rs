//! Wire types and request authentication shared by the relay and the client core.
//! Nothing in here is cryptography of our own: Ed25519 (ed25519-dalek) and SHA-256 (sha2)
//! are used as black boxes to sign and verify canonical byte strings.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Largest single log entry the relay accepts. Far above a real message or commit (an envelope is at most
/// 192 KB, a group table 64 KB), and small enough that a sync response always holds several.
pub const MAX_MESSAGE_BYTES: usize = 256 * 1024;
/// Upper bound on one sync response, in encoded bytes. A response always carries at least one entry, so a
/// huge old entry cannot stop a group, and it can never approach the 25 MB a client reads from one response.
pub const SYNC_MAX_BYTES: u64 = 4 * 1024 * 1024;
/// Smallest budget a client may ask for after a response it could not read.
pub const SYNC_MIN_BYTES: u64 = 64 * 1024;
/// Most accounts one ACL request may name, so that one request cannot hold the relay's database for long.
pub const MAX_ACL_BATCH: usize = 256;
pub const MAX_BLOB_BYTES: usize = 25 * 1024 * 1024;
pub const MAX_WELCOME_BYTES: usize = 1024 * 1024;
pub const MAX_RENDEZVOUS_BYTES: usize = 8 * 1024;
pub const AUTH_SKEW_SECS: u64 = 600;

/// Entry tags: the first byte of every opaque log entry. The relay never parses beyond this.
pub const TAG_MLS: u8 = 1;
pub const TAG_REJOIN: u8 = 2;
pub const TAG_JOIN_REQUEST: u8 = 3;
/// A commit (membership/epoch change). Visible to the relay as a different tag; see THREAT-MODEL.md.
pub const TAG_COMMIT: u8 = 4;

#[derive(thiserror::Error, Debug)]
pub enum ProtoError {
    #[error("bad encoding: {0}")]
    Encoding(String),
    #[error("bad signature")]
    BadSignature,
    #[error("bad length")]
    Length,
    #[error("request timestamp is too far from the relay's clock")]
    ClockSkew,
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
pub fn b64(b: &[u8]) -> String { B64.encode(b) }
pub fn unb64(s: &str) -> Result<Vec<u8>, ProtoError> {
    B64.decode(s).map_err(|e| ProtoError::Encoding(e.to_string()))
}
pub fn unhex32(s: &str) -> Result<[u8; 32], ProtoError> {
    let v = hex::decode(s).map_err(|e| ProtoError::Encoding(e.to_string()))?;
    v.try_into().map_err(|_| ProtoError::Length)
}
pub fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    for p in parts { h.update((p.len() as u64).to_be_bytes()); h.update(p); }
    h.finalize().into()
}

/// Account A signs "this device key belongs to me". The MLS credential carries this cert.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceCert {
    pub account: String, // hex ed25519 public key
    pub device: String,  // hex ed25519 public key (also the MLS signature key)
    pub created: u64,
    pub sig: String, // hex, by the account key
}

fn cert_msg(account: &[u8; 32], device: &[u8; 32], created: u64) -> Vec<u8> {
    let mut m = b"commons/cert/v1".to_vec();
    m.extend_from_slice(account); m.extend_from_slice(device); m.extend_from_slice(&created.to_be_bytes());
    m
}

impl DeviceCert {
    pub fn issue(account_key: &SigningKey, device_pub: &[u8; 32]) -> Self {
        let created = now_secs();
        let ap = account_key.verifying_key().to_bytes();
        let sig = account_key.sign(&cert_msg(&ap, device_pub, created));
        DeviceCert { account: hex::encode(ap), device: hex::encode(device_pub), created, sig: hex::encode(sig.to_bytes()) }
    }
    pub fn verify(&self) -> Result<(), ProtoError> {
        let a = unhex32(&self.account)?;
        let d = unhex32(&self.device)?;
        let sig = hex::decode(&self.sig).map_err(|e| ProtoError::Encoding(e.to_string()))?;
        let sig = Signature::from_slice(&sig).map_err(|_| ProtoError::BadSignature)?;
        let vk = VerifyingKey::from_bytes(&a).map_err(|_| ProtoError::BadSignature)?;
        vk.verify(&cert_msg(&a, &d, self.created), &sig).map_err(|_| ProtoError::BadSignature)
    }
    pub fn to_bytes(&self) -> Vec<u8> { serde_json::to_vec(self).unwrap_or_default() }
    /// Parse + verify untrusted bytes (credential identity). Never panics.
    pub fn from_bytes(b: &[u8]) -> Result<Self, ProtoError> {
        if b.len() > 1024 { return Err(ProtoError::Length); }
        let c: DeviceCert = serde_json::from_slice(b).map_err(|e| ProtoError::Encoding(e.to_string()))?;
        c.verify()?;
        Ok(c)
    }
}

/// Revocation of a device, signed by the account key.
pub fn revoke_msg(device: &[u8; 32]) -> Vec<u8> {
    let mut m = b"commons/revoke/v1".to_vec(); m.extend_from_slice(device); m
}
pub fn sign_revocation(account_key: &SigningKey, device: &[u8; 32]) -> String {
    hex::encode(account_key.sign(&revoke_msg(device)).to_bytes())
}
pub fn verify_revocation(account: &str, device: &str, sig: &str) -> Result<(), ProtoError> {
    let a = unhex32(account)?; let d = unhex32(device)?;
    let sig = hex::decode(sig).map_err(|e| ProtoError::Encoding(e.to_string()))?;
    let sig = Signature::from_slice(&sig).map_err(|_| ProtoError::BadSignature)?;
    VerifyingKey::from_bytes(&a).map_err(|_| ProtoError::BadSignature)?
        .verify(&revoke_msg(&d), &sig).map_err(|_| ProtoError::BadSignature)
}

// ---- group authority, as far as the relay needs to see it ----
fn hex_sig(sig: &str) -> Result<Signature, ProtoError> {
    let b = hex::decode(sig).map_err(|e| ProtoError::Encoding(e.to_string()))?;
    Signature::from_slice(&b).map_err(|_| ProtoError::BadSignature)
}
fn vk_of(account: &str) -> Result<VerifyingKey, ProtoError> {
    VerifyingKey::from_bytes(&unhex32(account)?).map_err(|_| ProtoError::BadSignature)
}
/// What the community owner signs to make an account an admin. The same grant sits in the group's own table,
/// so a client proves "I am an admin here" to the relay with the bytes every member already verifies.
pub fn grant_msg(community: &str, account: &str) -> Vec<u8> {
    let mut m = b"commons/admin/v1".to_vec();
    m.extend_from_slice(&sha256(&[community.as_bytes(), account.as_bytes()])); m
}
pub fn verify_grant(owner: &str, community: &str, account: &str, sig: &str) -> Result<(), ProtoError> {
    vk_of(owner)?.verify(&grant_msg(community, account), &hex_sig(sig)?).map_err(|_| ProtoError::BadSignature)
}
/// What an owner signs to hand a community to someone else. `seq` is the position in the chain of owners, so an
/// old transfer cannot be replayed after the community has changed hands again.
pub fn transfer_msg(community: &str, seq: u64, from: &str, to: &str) -> Vec<u8> {
    let mut m = b"commons/owner/v1".to_vec();
    m.extend_from_slice(&sha256(&[community.as_bytes(), &seq.to_be_bytes(), from.as_bytes(), to.as_bytes()])); m
}
pub fn verify_transfer(community: &str, seq: u64, from: &str, to: &str, sig: &str) -> Result<(), ProtoError> {
    vk_of(from)?.verify(&transfer_msg(community, seq, from, to), &hex_sig(sig)?).map_err(|_| ProtoError::BadSignature)
}

// ---- request authentication: X-Commons-Auth: <device_hex>.<ts>.<sig_hex> ----
pub const AUTH_HEADER: &str = "x-commons-auth";
fn auth_msg(method: &str, path: &str, ts: u64, body: &[u8]) -> Vec<u8> {
    let mut m = b"commons/req/v1".to_vec();
    m.extend_from_slice(&sha256(&[method.as_bytes(), path.as_bytes(), &ts.to_be_bytes(), body]));
    m
}
pub fn sign_request(device_key: &SigningKey, method: &str, path: &str, body: &[u8]) -> String {
    let ts = now_secs();
    let sig = device_key.sign(&auth_msg(method, path, ts, body));
    format!("{}.{}.{}", hex::encode(device_key.verifying_key().to_bytes()), ts, hex::encode(sig.to_bytes()))
}
/// Returns the authenticated device id (hex).
pub fn verify_request(header: &str, method: &str, path: &str, body: &[u8]) -> Result<String, ProtoError> {
    let mut it = header.split('.');
    let (d, ts, sig) = (it.next().ok_or(ProtoError::Length)?, it.next().ok_or(ProtoError::Length)?, it.next().ok_or(ProtoError::Length)?);
    if it.next().is_some() { return Err(ProtoError::Length); }
    let ts: u64 = ts.parse().map_err(|_| ProtoError::Length)?;
    if now_secs().abs_diff(ts) > AUTH_SKEW_SECS { return Err(ProtoError::ClockSkew); }
    let dk = unhex32(d)?;
    let sig = hex::decode(sig).map_err(|e| ProtoError::Encoding(e.to_string()))?;
    let sig = Signature::from_slice(&sig).map_err(|_| ProtoError::BadSignature)?;
    VerifyingKey::from_bytes(&dk).map_err(|_| ProtoError::BadSignature)?
        .verify(&auth_msg(method, path, ts, body), &sig).map_err(|_| ProtoError::BadSignature)?;
    Ok(d.to_string())
}

// ---- relay API bodies (all opaque payloads are base64url) ----
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RegisterReq {
    pub cert: DeviceCert,
    /// The relay's invite secret, needed for a new account when the relay runs with a registration gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite: Option<String>,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RevokeReq { pub account: String, pub device: String, pub sig: String }
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DeviceInfo { pub cert: DeviceCert, pub revoked_sig: Option<String> }
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DevicesResp { pub devices: Vec<DeviceInfo> }
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct PublishKpReq { pub packages: Vec<String>, pub last_resort: Option<String> }
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct KpResp { pub package: Option<String> }
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct AclReq {
    pub add: Vec<String>,
    pub remove: Vec<String>,
    /// When claiming a new channel group: the control group it belongs to. Absent = this group is its own root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// The requester's owner-signed admin grant for the root group. The owner needs none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant: Option<String>,
    /// Owner only: accounts whose grant no longer counts (the grant itself cannot be recalled), and undoing that.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub demote: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub promote: Vec<String>,
    /// Accounts to cut off from call tokens at once (their read access keeps its grace period). Needs the same authority as `remove`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub call_remove: Vec<String>,
    /// A step of the ownership chain the relay has not seen yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer: Option<OwnerStep>,
}
/// Reply of `POST /v1/groups/{gid}/call-token`: everything the client needs to join the media room.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CallTokenResp { pub url: String, pub token: String, pub room: String, pub identity: String, pub expires: u64 }
/// The opaque media-room name for a group: the media server never sees the group id itself.
pub fn call_room(gid: &str) -> String { hex::encode(sha256(&[b"commons/livekit-room/v1", gid.as_bytes()])) }
/// One link of the chain of owners: `from` hands the community to `to` as step `seq` (1 = from the founder).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct OwnerStep { pub seq: u64, pub from: String, pub to: String, pub sig: String }
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct AclResp {
    /// Group's current owner as the relay understands it, and how many transfers it has seen.
    pub owner: String,
    pub owner_seq: u64,
    pub members: Vec<String>,
}
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct InfoReq { pub invite: String }
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct InfoResp {
    /// New accounts need the relay's invite secret.
    pub gated: bool,
    /// Only meaningful in answer to a POST carrying a secret.
    #[serde(default)]
    pub ok: bool,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PostReq { pub id: String, pub data: String }
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PostResp { pub seq: u64, pub duplicate: bool }
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct SyncReq {
    pub cursors: std::collections::BTreeMap<String, u64>,
    pub ack_welcomes: Vec<String>,
    pub wait_ms: u64,
    /// Budget for this response. A client lowers it after a response it could not read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Entry { pub seq: u64, pub id: String, pub data: String }
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WelcomeEntry { pub id: String, pub data: String }
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct SyncResp {
    pub messages: std::collections::BTreeMap<String, Vec<Entry>>,
    pub welcomes: Vec<WelcomeEntry>,
    pub denied: Vec<String>,
    /// Groups with entries beyond this response: the client is not caught up on them yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub more: Vec<String>,
}
