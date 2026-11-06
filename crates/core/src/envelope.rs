//! Everything that travels inside an MLS application message or group-context extension.
//! All parsing of untrusted bytes is here and must never panic (see tests/fuzz_parse.rs).
use serde::{Deserialize, Serialize};

pub const MAX_ENVELOPE: usize = 192 * 1024;
pub const MAX_TEXT: usize = 16 * 1024;
pub const EXT_COMMONS: u16 = 0xF0C1;

fn is_id(s: &str) -> bool { s.len() == 32 && s.bytes().all(|c| c.is_ascii_hexdigit()) }
fn short(s: &str, n: usize) -> bool { s.len() <= n && !s.chars().any(|c| c.is_control() && c != '\n' && c != '\t') }

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "t")]
pub enum Envelope {
    Text { id: String, text: String, #[serde(default)] reply: Option<String>, ts: u64 },
    Edit { target: String, text: String, ts: u64 },
    Delete { target: String },
    React { target: String, emoji: String, on: bool },
    File { id: String, name: String, mime: String, size: u64, blob: String, key: String, nonce: String, #[serde(default)] caption: String, #[serde(default)] reply: Option<String>, ts: u64 },
    Typing,
    Profile { name: String, ts: u64 },
    /// "I am leaving" (the leaver cannot remove itself in MLS; an admin commits the removal).
    Leave,
    /// Account-signed device revocation, so members can drop the leaf even if the relay hides it.
    Revoked { account: String, device: String, sig: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError(pub String);
impl std::fmt::Display for ParseError { fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { write!(f, "{}", self.0) } }
impl std::error::Error for ParseError {}
fn bad<T>(m: &str) -> Result<T, ParseError> { Err(ParseError(m.into())) }

impl Envelope {
    pub fn to_bytes(&self) -> Vec<u8> { serde_json::to_vec(self).unwrap_or_default() }
    pub fn parse(b: &[u8]) -> Result<Envelope, ParseError> {
        if b.len() > MAX_ENVELOPE { return bad("envelope too large"); }
        let e: Envelope = serde_json::from_slice(b).map_err(|e| ParseError(format!("malformed message: {e}")))?;
        e.validate()?;
        Ok(e)
    }
    pub fn validate(&self) -> Result<(), ParseError> {
        use Envelope::*;
        match self {
            Text { id, text, reply, .. } => {
                if !is_id(id) || text.len() > MAX_TEXT { return bad("bad text"); }
                if reply.as_deref().is_some_and(|r| !is_id(r)) { return bad("bad reply"); }
            }
            Edit { target, text, .. } => if !is_id(target) || text.len() > MAX_TEXT { return bad("bad edit"); },
            Delete { target } => if !is_id(target) { return bad("bad delete"); },
            React { target, emoji, .. } => if !is_id(target) || emoji.is_empty() || emoji.len() > 32 || !short(emoji, 32) { return bad("bad reaction"); },
            File { id, name, mime, size, blob, key, nonce, caption, reply, .. } => {
                if !is_id(id) || !is_id(blob) || !short(name, 255) || !short(mime, 100) || *size > commons_proto::MAX_BLOB_BYTES as u64 || caption.len() > MAX_TEXT { return bad("bad file"); }
                if key.len() != 64 || nonce.len() != 24 || hex::decode(key).is_err() || hex::decode(nonce).is_err() { return bad("bad file key"); }
                if reply.as_deref().is_some_and(|r| !is_id(r)) { return bad("bad reply"); }
            }
            Typing | Leave => {}
            Profile { name, .. } => if name.is_empty() || !short(name, 64) { return bad("bad profile"); },
            Revoked { account, device, sig } => if account.len() != 64 || device.len() != 64 || sig.len() != 128 { return bad("bad revocation"); },
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct InviteMeta {
    pub id: String,
    /// Ed25519 public key (hex). The matching secret key only exists inside the invite link.
    pub pk: String,
    pub expires: u64,
    /// None = unlimited.
    pub uses_left: Option<u32>,
}
/// Owner-signed admin certificate: lets anyone verify an admin against the pinned owner key.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct AdminGrant { pub account: String, pub sig: String }
pub fn community_id(owner: &str, nonce: &str) -> String {
    hex::encode(commons_proto::sha256(&[b"commons/community/v1", owner.as_bytes(), nonce.as_bytes()]))
}
fn grant_msg(community: &str, account: &str) -> Vec<u8> {
    let mut m = b"commons/admin/v1".to_vec();
    m.extend_from_slice(&commons_proto::sha256(&[community.as_bytes(), account.as_bytes()])); m
}
pub fn sign_grant(owner_key: &ed25519_dalek::SigningKey, community: &str, account: &str) -> AdminGrant {
    use ed25519_dalek::Signer;
    AdminGrant { account: account.into(), sig: hex::encode(owner_key.sign(&grant_msg(community, account)).to_bytes()) }
}
impl GroupExt {
    /// Full authenticity check for a control-group table against the community id (which pins the owner).
    /// Returns the verified admin account list.
    pub fn verify_control(&self, gid: &str) -> Result<(String, Vec<String>), ParseError> {
        use ed25519_dalek::Verifier;
        let GroupExt::Control { owner, nonce, admins, .. } = self else { return bad("not a control table") };
        if community_id(owner, nonce) != gid { return bad("community id does not match owner and nonce"); }
        let ok = commons_proto::unhex32(owner).ok().and_then(|o| ed25519_dalek::VerifyingKey::from_bytes(&o).ok()).ok_or(ParseError("bad owner key".into()))?;
        let mut out = vec![];
        for a in admins {
            let sig = hex::decode(&a.sig).ok().and_then(|s| ed25519_dalek::Signature::from_slice(&s).ok()).ok_or(ParseError("bad admin grant".into()))?;
            ok.verify(&grant_msg(gid, &a.account), &sig).map_err(|_| ParseError("admin grant not signed by owner".into()))?;
            out.push(a.account.clone());
        }
        Ok((owner.clone(), out))
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ChanMeta { pub id: String, pub name: String }

/// Stored in the group-context extension of every group, so it is authenticated by MLS
/// and handed to a joining device inside the Welcome.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "k")]
pub enum GroupExt {
    /// The community control group: roster = its members; this is the roles/channels/invites table.
    Control { name: String, owner: String, nonce: String, admins: Vec<AdminGrant>, channels: Vec<ChanMeta>, invites: Vec<InviteMeta>, #[serde(default)] banned: Vec<String> },
    /// A channel group. `community` is the control group id; `name` is only a hint, the control table is authoritative.
    Channel { community: String, name: String },
}

impl GroupExt {
    pub fn to_bytes(&self) -> Vec<u8> { serde_json::to_vec(self).unwrap_or_default() }
    pub fn parse(b: &[u8]) -> Result<GroupExt, ParseError> {
        if b.len() > 64 * 1024 { return bad("group ext too large"); }
        let g: GroupExt = serde_json::from_slice(b).map_err(|e| ParseError(format!("bad group ext: {e}")))?;
        match &g {
            GroupExt::Control { name, owner, nonce, admins, channels, invites, banned } => {
                if banned.len() > 2000 { return bad("too many bans"); }
                if name.is_empty() || !short(name, 64) || owner.len() != 64 || nonce.len() != 32 || admins.len() > 512 || channels.len() > 500 || invites.len() > 200 { return bad("bad control ext"); }
                for c in channels { if c.id.len() < 8 || !short(&c.name, 64) || c.name.is_empty() { return bad("bad channel"); } }
                for i in invites { if !is_id(&i.id) || i.pk.len() != 64 { return bad("bad invite"); } }
            }
            GroupExt::Channel { community, name } => if community.len() < 8 || !short(name, 64) { return bad("bad channel ext"); },
        }
        Ok(g)
    }
}

/// Join request placed (in the clear, signed) into the control group's log by a not-yet-member.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct JoinRequest {
    pub invite: String,
    pub cert: commons_proto::DeviceCert,
    pub sig: String, // by the invite secret key over join_msg()
}
pub fn join_msg(community: &str, device: &str, invite: &str) -> Vec<u8> {
    let mut m = b"commons/join/v1".to_vec();
    m.extend_from_slice(&commons_proto::sha256(&[community.as_bytes(), device.as_bytes(), invite.as_bytes()]));
    m
}
impl JoinRequest {
    pub fn parse(b: &[u8]) -> Result<Self, ParseError> {
        if b.len() > 4096 { return bad("join request too large"); }
        let j: JoinRequest = serde_json::from_slice(b).map_err(|e| ParseError(format!("bad join: {e}")))?;
        if !is_id(&j.invite) || j.sig.len() != 128 { return bad("bad join"); }
        j.cert.verify().map_err(|_| ParseError("bad cert".into()))?;
        Ok(j)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct InviteLink { pub owner: String, pub relay: String, pub community: String, pub invite: String, pub sk: String, pub name: String }
impl InviteLink {
    pub fn encode(&self) -> String { format!("commons://join/{}", commons_proto::b64(&serde_json::to_vec(self).unwrap_or_default())) }
    pub fn decode(s: &str) -> Result<Self, ParseError> {
        let s = s.trim().strip_prefix("commons://join/").ok_or(ParseError("not a commons invite link".into()))?;
        let b = commons_proto::unb64(s).map_err(|_| ParseError("bad invite link".into()))?;
        if b.len() > 4096 { return bad("invite too large"); }
        let l: InviteLink = serde_json::from_slice(&b).map_err(|_| ParseError("bad invite link".into()))?;
        if !is_id(&l.invite) || l.sk.len() != 64 || l.owner.len() != 64 || l.community.len() < 8 || !(l.relay.starts_with("http://") || l.relay.starts_with("https://")) { return bad("bad invite link"); }
        Ok(l)
    }
}
