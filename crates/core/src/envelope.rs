//! Everything that travels inside an MLS application message or group-context extension.
//! All parsing of untrusted bytes is here and must never panic (see tests/fuzz_parse.rs).
use commons_proto::OwnerStep;
use serde::{Deserialize, Serialize};

pub const MAX_ENVELOPE: usize = 192 * 1024;
pub const MAX_TEXT: usize = 16 * 1024;
pub const EXT_COMMONS: u16 = 0xF0C1;

pub fn is_id(s: &str) -> bool { s.len() == 32 && s.bytes().all(|c| c.is_ascii_hexdigit()) }
fn is_gid(s: &str) -> bool { s.len() >= 8 && s.len() <= 128 && s.bytes().all(|c| c.is_ascii_hexdigit()) }
fn is_key(s: &str) -> bool { s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()) }
fn short(s: &str, n: usize) -> bool { s.len() <= n && !s.chars().any(|c| c.is_control() && c != '\n' && c != '\t') }
/// One line of text with no control characters at all: what a shared song title or artist may be.
fn one_line(s: &str, n: usize) -> bool { s.len() <= n && !s.chars().any(char::is_control) }
/// Most distinct reactions on one message, and most from one account: enough for any real conversation, and it
/// stops one member from growing everybody's state and timeline without end.
pub const MAX_REACTIONS: usize = 32;
pub const MAX_REACTIONS_PER_ACCOUNT: usize = 8;

/// type/subtype in lower case, nothing else: `^[a-z0-9.+-]+/[a-z0-9.+-]+$`. A mime type ends up in a data or blob
/// URL and in markup, so it must not be able to carry anything but a mime type.
pub fn valid_mime(m: &str) -> bool {
    let part = |p: &str| !p.is_empty() && p.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'+' | b'-'));
    m.len() <= 100 && m.split_once('/').is_some_and(|(a, b)| part(a) && part(b))
}
/// What a sender does with whatever the file picker reported.
pub fn clean_mime(m: &str) -> String {
    let m = m.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    if valid_mime(&m) { m } else { "application/octet-stream".into() }
}
/// A person's, community's or room's name, already trimmed: one line, 1-64 bytes.
pub fn check_name(what: &str, n: &str) -> Result<(), String> {
    if n.is_empty() { return Err(format!("{what} cannot be empty")); }
    if n.len() > 64 { return Err(format!("{what} is too long (64 bytes at most; this one is {})", n.len())); }
    if n.chars().any(|c| c.is_control()) { return Err(format!("{what} cannot contain line breaks or control characters")); }
    Ok(())
}
/// Names that look the same to a person compare equal: case, spacing and invisible characters do not count.
pub fn name_key(n: &str) -> String {
    n.chars().filter(|c| !c.is_whitespace() && !c.is_control() && !matches!(*c, '\u{ad}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2069}' | '\u{feff}')).flat_map(char::to_lowercase).collect()
}
/// A file name that is safe to hand to a save dialog on any system: no path, no hidden file, no reversed text.
pub fn safe_file_name(n: &str) -> String {
    let s: String = n.chars().filter(|c| !c.is_control() && !matches!(*c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c }).collect();
    let s = s.trim().trim_start_matches('.').trim_end_matches(['.', ' ']);
    let s = cut_name(s, 200);
    let stem = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit());
    if s.is_empty() { "file".into() } else if reserved { format!("_{s}") } else { s }
}
/// At most `max` bytes, cut on a character boundary, keeping a short extension.
pub fn cut_name(n: &str, max: usize) -> String {
    if n.len() <= max { return n.to_string(); }
    let ext = n.rfind('.').map(|i| &n[i..]).filter(|e| e.len() <= 16).unwrap_or("");
    let mut end = max - ext.len();
    while !n.is_char_boundary(end) { end -= 1; }
    format!("{}{ext}", &n[..end])
}

fn is_false(b: &bool) -> bool { !*b }
/// What someone is listening to in rune. Only two short lines of text: the same whether the track came from
/// Spotify or from the local library.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct NowPlaying { pub title: String, #[serde(default)] pub artist: String }
/// How long a shared status lasts without a fresh one, and how often a playing sender renews it. The gap leaves
/// room for a missed refresh before friends stop believing the last one.
pub const LISTENING_TTL_SECS: u64 = 600;
pub const LISTENING_REFRESH_SECS: u64 = 300;
pub const MAX_LISTEN_LINE: usize = 160;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "t")]
pub enum Envelope {
    Text { id: String, text: String, #[serde(default)] reply: Option<String>, ts: u64 },
    Edit { target: String, text: String, ts: u64 },
    Delete { target: String },
    React { target: String, emoji: String, on: bool },
    File { id: String, name: String, mime: String, size: u64, blob: String, key: String, nonce: String, #[serde(default)] caption: String, #[serde(default)] reply: Option<String>, ts: u64 },
    Typing,
    /// A profile refresh. `listen` rides along as an extension field: an older Megatron reads this as the profile
    /// update it has always been and ignores the field, so a shared status never becomes a message kind it does not
    /// know (which would put a notice in the room every time the song changed). `None` means "nothing to share".
    Profile { name: String, ts: u64, #[serde(default)] listen: Option<NowPlaying> },
    /// "I am leaving" (the leaver cannot remove itself in MLS; an admin commits the removal).
    Leave,
    /// Account-signed device revocation, so members can drop the leaf even if the relay hides it.
    Revoked { account: String, device: String, sig: String },
    /// A member's own `Consent` for this community, for an admin to put in the table. Only for members whose table entry
    /// is missing: everyone else's arrived with their join request.
    ///
    /// The two optional fields are how one of a person's devices hands the relay's sign-up secret to another (see
    /// `Client::seal_relay_secret`). They ride on `Consent` on purpose: an app from before they existed parses this as
    /// an ordinary consent and checks the signature, so it neither reports an unknown kind nor acts on the extra data.
    Consent {
        ts: u64,
        sig: String,
        /// The sign-up secret, sealed to the account's own devices. Other members can read the bytes and nothing else.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        relay_secret: Option<String>,
        /// This account's device has no secret and is asking its other devices for it.
        #[serde(default, skip_serializing_if = "is_false")]
        relay_secret_want: bool,
    },
    /// A kind this version does not know, sent by a newer app. Skipped quietly instead of shown as a broken message, so
    /// that a newer friend's app adding a kind never makes an older app's rooms look damaged (docs/DESIGN.md, compatibility).
    /// Never sent.
    #[serde(other)]
    Unknown,
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
            React { target, emoji, .. } => {
                // Emoji, not text: ASCII only appears in keycaps such as 1️⃣ and #️⃣.
                let emojiish = emoji.chars().all(|c| !c.is_ascii() || c.is_ascii_digit() || c == '#' || c == '*') && emoji.chars().any(|c| !c.is_ascii());
                if !is_id(target) || emoji.is_empty() || emoji.chars().count() > 16 || !short(emoji, 32) || !emojiish { return bad("bad reaction"); }
            }
            File { id, name, mime, size, blob, key, nonce, caption, reply, .. } => {
                if !is_id(id) || !is_id(blob) || !short(name, 255) || !valid_mime(mime) || *size > commons_proto::MAX_BLOB_BYTES as u64 || caption.len() > MAX_TEXT { return bad("bad file"); }
                if key.len() != 64 || nonce.len() != 24 || hex::decode(key).is_err() || hex::decode(nonce).is_err() { return bad("bad file key"); }
                if reply.as_deref().is_some_and(|r| !is_id(r)) { return bad("bad reply"); }
            }
            Typing | Leave | Unknown => {}
            Profile { name, listen, .. } => {
                if name.is_empty() || !short(name, 64) { return bad("bad profile"); }
                if let Some(l) = listen { if l.title.is_empty() || !one_line(&l.title, MAX_LISTEN_LINE) || !one_line(&l.artist, MAX_LISTEN_LINE) { return bad("bad listening status"); } }
            }
            Revoked { account, device, sig } => if !is_key(account) || !is_key(device) || sig.len() != 128 { return bad("bad revocation"); },
            Consent { sig, relay_secret, .. } => {
                if sig.len() != 128 { return bad("bad consent"); }
                // Sealed secret: a base64 blob of a nonce and ciphertext, never more than a short secret needs.
                if relay_secret.as_ref().is_some_and(|s| s.len() > 1024 || commons_proto::unb64(s).is_err()) { return bad("bad relay secret"); }
            }
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
use commons_proto::grant_msg;
pub fn sign_grant(owner_key: &ed25519_dalek::SigningKey, community: &str, account: &str) -> AdminGrant {
    use ed25519_dalek::Signer;
    AdminGrant { account: account.into(), sig: hex::encode(owner_key.sign(&grant_msg(community, account)).to_bytes()) }
}
/// Most consents one table holds. Past this a community still works; its newest members just cannot add a device
/// without a fresh invite link.
pub const MAX_CONSENTS: usize = 300;
/// "This account agreed to be in this community", signed with the account key, which never leaves the person's own
/// devices. It is what lets a device with no history of its own (a second device, or one made from the recovery key)
/// accept a Welcome: nothing an admin of some other community can produce. See DESIGN.md, "Who may put you in a community".
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Consent { pub account: String, pub ts: u64, pub sig: String }
pub fn consent_msg(community: &str, account: &str, ts: u64) -> Vec<u8> {
    let mut m = b"commons/consent/v1".to_vec();
    m.extend_from_slice(&commons_proto::sha256(&[community.as_bytes(), account.as_bytes(), &ts.to_be_bytes()]));
    m
}
pub fn sign_consent(account_key: &ed25519_dalek::SigningKey, community: &str, ts: u64) -> Consent {
    use ed25519_dalek::Signer;
    let account = hex::encode(account_key.verifying_key().to_bytes());
    let sig = hex::encode(account_key.sign(&consent_msg(community, &account, ts)).to_bytes());
    Consent { account, ts, sig }
}
pub fn verify_consent(community: &str, c: &Consent) -> Result<(), ParseError> {
    use ed25519_dalek::Verifier;
    let vk = commons_proto::unhex32(&c.account).ok().and_then(|a| ed25519_dalek::VerifyingKey::from_bytes(&a).ok()).ok_or(ParseError("bad account key".into()))?;
    let sig = hex::decode(&c.sig).ok().and_then(|s| ed25519_dalek::Signature::from_slice(&s).ok()).ok_or(ParseError("bad signature".into()))?;
    vk.verify(&consent_msg(community, &c.account, c.ts), &sig).map_err(|_| ParseError("consent not signed by that account".into()))
}
/// Put a consent in a list: one per account, the newest wins. True if the list changed.
pub fn merge_consent(list: &mut Vec<Consent>, c: Consent) -> bool {
    if let Some(old) = list.iter_mut().find(|x| x.account == c.account) {
        if old.ts >= c.ts { return false; }
        *old = c;
        return true;
    }
    if list.len() >= MAX_CONSENTS { return false; }
    list.push(c);
    true
}

/// Every key that has owned the community, founder first, each step checked. The last one is the owner now.
/// The founder stays pinned in the community id; a transfer is a signed step away from it.
pub fn owner_chain(founder: &str, community: &str, transfers: &[OwnerStep]) -> Result<Vec<String>, ParseError> {
    let mut chain = vec![founder.to_string()];
    for (i, t) in transfers.iter().enumerate() {
        let (seq, from) = (i as u64 + 1, chain.last().unwrap());
        if t.seq != seq || &t.from != from || !is_key(&t.to) || commons_proto::verify_transfer(community, seq, &t.from, &t.to, &t.sig).is_err() { return bad("bad ownership transfer"); }
        chain.push(t.to.clone());
    }
    Ok(chain)
}
/// A grant counts if any owner in the chain signed it: that keeps the admins in place across a transfer, until the
/// new owner has re-signed them. Who may *change* the list is decided by the commit rules, not here.
fn check_grants(community: &str, chain: &[String], admins: &[AdminGrant]) -> Result<Vec<String>, ParseError> {
    let mut out = vec![];
    for a in admins {
        if !chain.iter().any(|o| commons_proto::verify_grant(o, community, &a.account, &a.sig).is_ok()) { return bad("admin grant not signed by owner"); }
        out.push(a.account.clone());
    }
    Ok(out)
}
impl GroupExt {
    /// Full authenticity check for a control-group table against the community id (which pins the founder).
    /// Returns the current owner and the verified admin account list.
    pub fn verify_control(&self, gid: &str) -> Result<(String, Vec<String>), ParseError> {
        let GroupExt::Control { owner, nonce, admins, transfers, .. } = self else { return bad("not a control table") };
        if community_id(owner, nonce) != gid { return bad("community id does not match owner and nonce"); }
        let chain = owner_chain(owner, gid, transfers)?;
        Ok((chain.last().unwrap().clone(), check_grants(gid, &chain, admins)?))
    }
    /// Same for a channel group: the signed admin list embedded in the channel itself, so that the authorization
    /// of a channel commit depends on nothing but that channel's own authenticated state.
    pub fn verify_channel(&self) -> Result<(String, String, Vec<String>), ParseError> {
        let GroupExt::Channel { community, owner, nonce, admins, transfers, .. } = self else { return bad("not a channel table") };
        if community_id(owner, nonce) != *community { return bad("channel does not belong to the community its owner/nonce name"); }
        let chain = owner_chain(owner, community, transfers)?;
        Ok((community.clone(), chain.last().unwrap().clone(), check_grants(community, &chain, admins)?))
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
    /// `owner` is the founder, pinned in the community id. `transfers` is the signed chain from there to today's owner.
    Control { name: String, owner: String, nonce: String, admins: Vec<AdminGrant>, channels: Vec<ChanMeta>, invites: Vec<InviteMeta>, #[serde(default)] banned: Vec<String>, #[serde(default, skip_serializing_if = "Vec::is_empty")] transfers: Vec<OwnerStep>, #[serde(default, skip_serializing_if = "Vec::is_empty")] consents: Vec<Consent> },
    /// A channel group. `community` is the control group id; `name` is only a hint, the control table is authoritative.
    Channel { community: String, name: String, owner: String, nonce: String, admins: Vec<AdminGrant>, #[serde(default, skip_serializing_if = "Vec::is_empty")] transfers: Vec<OwnerStep> },
}

impl GroupExt {
    pub fn to_bytes(&self) -> Vec<u8> { serde_json::to_vec(self).unwrap_or_default() }
    pub fn parse(b: &[u8]) -> Result<GroupExt, ParseError> {
        if b.len() > 128 * 1024 { return bad("group ext too large"); }
        let g: GroupExt = serde_json::from_slice(b).map_err(|e| ParseError(format!("bad group ext: {e}")))?;
        match &g {
            GroupExt::Control { name, owner, nonce, admins, channels, invites, banned, transfers, consents } => {
                if banned.len() > 2000 || transfers.len() > 200 { return bad("too many bans or transfers"); }
                if consents.len() > MAX_CONSENTS || consents.iter().any(|c| !is_key(&c.account) || c.sig.len() != 128) { return bad("bad consents"); }
                if name.is_empty() || !short(name, 64) || !is_key(owner) || nonce.len() != 32 || admins.len() > 512 || channels.len() > 500 || invites.len() > 200 { return bad("bad control ext"); }
                // A channel id goes into URLs and into markup: hex only, whoever wrote this table.
                for c in channels { if !is_gid(&c.id) || !short(&c.name, 64) || c.name.is_empty() { return bad("bad channel"); } }
                for i in invites { if !is_id(&i.id) || !is_key(&i.pk) { return bad("bad invite"); } }
            }
            GroupExt::Channel { community, name, owner, nonce, admins, transfers } => if !is_gid(community) || !short(name, 64) || !is_key(owner) || nonce.len() != 32 || admins.len() > 512 || transfers.len() > 200 { return bad("bad channel ext"); },
        }
        Ok(g)
    }
}

/// Why a join will not happen. Codes, not text: whoever sends one does not get to put words on the joiner's screen.
pub const JOIN_INVITE_GONE: &str = "invite-gone";
pub const JOIN_BANNED: &str = "banned";
pub fn join_reason_text(code: &str) -> &'static str {
    match code {
        JOIN_BANNED => "you are banned from this community",
        JOIN_INVITE_GONE => "that invite is no longer valid (it was used up, revoked, or it expired); ask for a new one",
        _ => "the community refused the request",
    }
}
/// An admin's answer to a join request it cannot grant, sent to the joiner's inbox: the joiner is not in any group
/// yet, so there is no other way to reach them. It proves its author is an admin of the community the invite link
/// names, so that a stranger cannot talk somebody out of joining.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct JoinReply {
    pub community: String,
    pub reason: String,
    pub cert: commons_proto::DeviceCert,
    /// Signature by the admin's device key.
    pub sig: String,
    /// The admin's grant; absent if they are the owner.
    #[serde(default)]
    pub grant: Option<String>,
    #[serde(default)]
    pub transfers: Vec<OwnerStep>,
}
pub fn join_reply_msg(community: &str, device: &str, reason: &str) -> Vec<u8> {
    let mut m = b"commons/join-reply/v1".to_vec();
    m.extend_from_slice(&commons_proto::sha256(&[community.as_bytes(), device.as_bytes(), reason.as_bytes()]));
    m
}
impl JoinReply {
    pub fn parse(b: &[u8]) -> Result<Self, ParseError> {
        if b.len() > 64 * 1024 { return bad("join reply too large"); }
        let j: JoinReply = serde_json::from_slice(b).map_err(|e| ParseError(format!("bad join reply: {e}")))?;
        if !is_gid(&j.community) || j.reason.len() > 32 || j.sig.len() != 128 { return bad("bad join reply"); }
        j.cert.verify().map_err(|_| ParseError("bad cert".into()))?;
        Ok(j)
    }
    /// Checks a reply addressed to `device` against the founder in the invite link.
    pub fn verify(&self, founder: &str, device: &str) -> Result<(), ParseError> {
        use ed25519_dalek::Verifier;
        let sig = hex::decode(&self.sig).ok().and_then(|s| ed25519_dalek::Signature::from_slice(&s).ok()).ok_or(ParseError("bad signature".into()))?;
        let dk = commons_proto::unhex32(&self.cert.device).ok().and_then(|d| ed25519_dalek::VerifyingKey::from_bytes(&d).ok()).ok_or(ParseError("bad key".into()))?;
        dk.verify(&join_reply_msg(&self.community, device, &self.reason), &sig).map_err(|_| ParseError("bad signature".into()))?;
        let chain = owner_chain(founder, &self.community, &self.transfers)?;
        if chain.last() == Some(&self.cert.account) { return Ok(()); }
        let g = self.grant.as_deref().ok_or(ParseError("not from an admin".into()))?;
        if chain.iter().any(|o| commons_proto::verify_grant(o, &self.community, &self.cert.account, g).is_ok()) { Ok(()) } else { bad("not from an admin") }
    }
}

/// Join request placed (in the clear, signed) into the control group's log by a not-yet-member.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct JoinRequest {
    pub invite: String,
    pub cert: commons_proto::DeviceCert,
    pub sig: String, // by the invite secret key over join_msg()
    /// The joiner's account agrees to be in this community. An admin copies it into the table.
    #[serde(default)]
    pub consent: Option<Consent>,
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
pub struct InviteLink {
    pub owner: String, pub relay: String, pub community: String, pub invite: String, pub sk: String, pub name: String,
    /// The relay's invite secret, so that a friend who has no account yet needs this one link and nothing else.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub secret: String,
}
/// A web address of the relay's /join page carries the link in its fragment, which browsers keep to themselves.
fn link_of(s: &str) -> String {
    let s = s.trim();
    match s.split_once("/join#") { Some((base, frag)) if base.starts_with("http") => format!("commons://{frag}"), _ => s.to_string() }
}
impl InviteLink {
    pub fn encode(&self) -> String { format!("commons://join/{}", commons_proto::b64(&serde_json::to_vec(self).unwrap_or_default())) }
    /// The same link as a page a friend can open before they have the app. Only over https: the link is a secret.
    pub fn web(&self) -> Option<String> {
        self.relay.starts_with("https://").then(|| format!("{}/join#join/{}", self.relay.trim_end_matches('/'), commons_proto::b64(&serde_json::to_vec(self).unwrap_or_default())))
    }
    pub fn decode(s: &str) -> Result<Self, ParseError> {
        let s = link_of(s);
        let s = s.strip_prefix("commons://join/").ok_or(ParseError("not a commons invite link".into()))?;
        let b = commons_proto::unb64(s).map_err(|_| ParseError("bad invite link".into()))?;
        if b.len() > 4096 { return bad("invite too large"); }
        let l: InviteLink = serde_json::from_slice(&b).map_err(|_| ParseError("bad invite link".into()))?;
        // These go into URLs and keys: a link is pasted from outside, so nothing about it is trusted.
        if !is_id(&l.invite) || !is_key(&l.sk) || !is_key(&l.owner) || !is_gid(&l.community) || !short(&l.name, 64) || !short(&l.secret, 256) || !(l.relay.starts_with("http://") || l.relay.starts_with("https://")) { return bad("bad invite link"); }
        Ok(l)
    }
}

/// What `deploy/go-online.sh` prints: where the relay is and how to get an account on it, with no community attached.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RelayLink { pub relay: String, #[serde(default)] pub secret: String }
impl RelayLink {
    pub fn decode(s: &str) -> Result<Self, ParseError> {
        let s = link_of(s);
        let s = s.strip_prefix("commons://relay/").ok_or(ParseError("not a relay link".into()))?;
        let b = commons_proto::unb64(s).map_err(|_| ParseError("bad relay link".into()))?;
        if b.len() > 1024 { return bad("relay link too large"); }
        let l: RelayLink = serde_json::from_slice(&b).map_err(|_| ParseError("bad relay link".into()))?;
        if !short(&l.secret, 256) || !(l.relay.starts_with("http://") || l.relay.starts_with("https://")) { return bad("bad relay link"); }
        Ok(l)
    }
}

#[cfg(test)]
mod compat_tests {
    use super::*;
    #[test]
    fn a_newer_app_can_add_fields_and_kinds() {
        // a field this version does not know is ignored
        let e = Envelope::parse(br#"{"t":"Text","id":"0123456789abcdef0123456789abcdef","text":"hi","ts":5,"sticker":"cat"}"#).unwrap();
        assert!(matches!(e, Envelope::Text { ref text, .. } if text == "hi"));
        // a kind this version does not know is recognised as such, so it can be skipped quietly
        assert_eq!(Envelope::parse(br#"{"t":"Poll","question":"pizza?","options":["yes","no"]}"#).unwrap(), Envelope::Unknown);
        assert_eq!(Envelope::parse(br#"{"t":"Typing"}"#).unwrap(), Envelope::Typing);
        // but what this version does know is still checked as strictly as before
        assert!(Envelope::parse(br#"{"t":"Text","id":"x","text":"hi","ts":5}"#).is_err());
        assert!(Envelope::parse(br#"{"text":"no kind"}"#).is_err());
        // and the old wire form is unchanged
        assert_eq!(Envelope::Typing.to_bytes(), br#"{"t":"Typing"}"#);
    }

    #[test]
    fn the_relay_secret_rides_on_a_shape_old_apps_already_know() {
        // What a device with the relay's sign-up secret sends. The extra fields are not a new kind, so an app from
        // 0.2.x must parse this as the Consent it knows rather than as Unknown (which it would show as a notice).
        let sig = "ab".repeat(64);
        let msg = format!(r#"{{"t":"Consent","ts":7,"sig":"{sig}","relay_secret":"AQID","relay_secret_want":true}}"#);
        let e = Envelope::parse(msg.as_bytes()).unwrap();
        assert!(matches!(e, Envelope::Consent { ts: 7, relay_secret: Some(ref s), relay_secret_want: true, .. } if s == "AQID"));
        assert_ne!(e, Envelope::Unknown);

        #[derive(serde::Deserialize)]
        #[serde(tag = "t")]
        enum OldEnvelope {
            Consent { ts: u64, sig: String },
            #[serde(other)]
            Unknown,
        }
        let old: OldEnvelope = serde_json::from_str(&msg).unwrap();
        assert!(matches!(old, OldEnvelope::Consent { ts: 7, ref sig } if sig == &"ab".repeat(64)));

        // An ordinary consent goes out exactly as before: no empty fields, no new spelling.
        let plain = Envelope::Consent { ts: 7, sig: sig.clone(), relay_secret: None, relay_secret_want: false };
        assert_eq!(plain.to_bytes(), format!(r#"{{"t":"Consent","ts":7,"sig":"{sig}"}}"#).into_bytes());
        // And an old consent, with neither field, still reads.
        assert_eq!(Envelope::parse(format!(r#"{{"t":"Consent","ts":7,"sig":"{sig}"}}"#).as_bytes()).unwrap(), plain);
    }

    #[test]
    fn a_profile_may_carry_a_listening_status() {
        let e = Envelope::parse(br#"{"t":"Profile","name":"Ann","ts":5,"listen":{"title":"A song","artist":"A band"}}"#).unwrap();
        assert!(matches!(e, Envelope::Profile { listen: Some(ref l), .. } if l.title == "A song" && l.artist == "A band"));
        // The profile an older version sends has no such field, and still reads.
        assert!(matches!(Envelope::parse(br#"{"t":"Profile","name":"Ann","ts":5}"#).unwrap(), Envelope::Profile { listen: None, .. }));
        // What one sends, an older version can read as a profile: nothing in it may be unprintable.
        assert!(Envelope::parse(br#"{"t":"Profile","name":"Ann","ts":5,"listen":{"title":"two\nlines"}}"#).is_err());
    }
}
