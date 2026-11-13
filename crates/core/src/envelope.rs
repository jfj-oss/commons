//! Everything that travels inside an MLS application message or group-context extension.
//! All parsing of untrusted bytes is here and must never panic (see tests/fuzz_parse.rs).
use commons_proto::OwnerStep;
use serde::{Deserialize, Serialize};

pub const MAX_ENVELOPE: usize = 192 * 1024;
pub const MAX_TEXT: usize = 16 * 1024;
/// A profile picture is shrunk on the device before it is sent (256x256, about 40 KB). This is the hard ceiling for
/// what this version will accept from a newer one, so a bad or careless sender cannot grow everybody's state.
pub const MAX_AVATAR: u64 = 256 * 1024;
pub const EXT_COMMONS: u16 = 0xF0C1;
/// Most bytes of `data` in one live event.
pub const MAX_LIVE: usize = 4096;
/// A live event older than this (by its sender's clock) is dropped on arrival: it is a replay, not live.
pub const LIVE_MAX_AGE: u64 = 30;
/// How far ahead of our own clock a live event may claim to be.
pub const LIVE_MAX_AHEAD: u64 = 60;
/// A ring older than this (by its sender's clock) is dropped on arrival: a device catching up on a log must never ring
/// for a call that ended long ago. It is longer than a ring lasts, so ordinary delay and a little clock drift still ring.
pub const RING_MAX_AGE: u64 = 45;
pub const RING_MAX_AHEAD: u64 = 60;
/// How long a ring lasts on the receiving screen, unless it is answered, declined or taken back first.
pub const RING_TTL: u64 = 30;
/// Most rings one person may start per minute (everywhere), and the most "taken back" messages we count for them.
pub const RING_PER_MIN: usize = 4;
pub const RING_ENDS_PER_MIN: usize = 8;
/// What a receiver does with one ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingVerdict { Show, Stale, Mine, NotMember, NotForMe, TooMany }
/// The receiving side's rules for a ring, as one pure function so they can be tested without a network.
/// `recent` = times (seconds) of rings already taken from this sender in the last minute.
pub fn ring_check(now: u64, ts: u64, sender: &str, me: &str, to: Option<&str>, sender_is_member: bool, recent: &[u64]) -> RingVerdict {
    if sender == me { return RingVerdict::Mine; }                       // never ring yourself or your other devices
    if !sender_is_member { return RingVerdict::NotMember; }
    if ts + RING_MAX_AGE < now || ts > now + RING_MAX_AHEAD { return RingVerdict::Stale; }
    if to.is_some_and(|t| t != me) { return RingVerdict::NotForMe; }
    if recent.iter().filter(|t| **t + 60 > now).count() >= RING_PER_MIN { return RingVerdict::TooMany; }
    RingVerdict::Show
}
pub fn valid_live_kind(k: &str) -> bool { !k.is_empty() && k.len() <= 32 && k.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')) }

pub fn is_id(s: &str) -> bool { s.len() == 32 && s.bytes().all(|c| c.is_ascii_hexdigit()) }
pub(crate) fn is_gid(s: &str) -> bool { s.len() >= 8 && s.len() <= 128 && s.bytes().all(|c| c.is_ascii_hexdigit()) }
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
pub struct NowPlaying {
    pub title: String,
    #[serde(default)] pub artist: String,
    /// Only for a Spotify song (0.3.7 on): what a friend needs to listen along. Older apps ignore the field (no
    /// `deny_unknown_fields` anywhere), so they show the title and artist exactly as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spotify: Option<SpotifyPlay>,
}
/// Where a shared Spotify song is: the track, the position at one instant of the sharer's clock, and whether it is paused.
/// A receiver uses the instants only relative to each other (clocks differ); see `spotify::expected_pos`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct SpotifyPlay {
    /// The 22-character Spotify track id.
    pub id: String,
    pub pos_ms: u64,
    /// The sharer's clock (ms since 1970) when `pos_ms` was true.
    pub at_ms: u64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub paused: bool,
}
impl SpotifyPlay {
    pub fn valid(&self) -> bool { is_spotify_id(&self.id) && self.pos_ms <= 24 * 3600 * 1000 }
}
pub fn is_spotify_id(s: &str) -> bool { s.len() == 22 && s.bytes().all(|c| c.is_ascii_alphanumeric()) }
/// How far a position may differ from where steady playing would have put it before it counts as a seek.
pub const SEEK_SLACK_MS: u64 = 2500;
impl NowPlaying {
    pub fn text(title: &str, artist: &str) -> NowPlaying { NowPlaying { title: title.into(), artist: artist.into(), spotify: None } }
    /// Is `self` worth telling friends about, given what they were last told? A new song, a pause or resume, or a seek
    /// (a position that steady playing since `prev` would not have reached). Plain progress is not news.
    pub fn is_news_after(&self, prev: &NowPlaying) -> bool {
        if self.title != prev.title || self.artist != prev.artist { return true; }
        match (&self.spotify, &prev.spotify) {
            (None, None) => false,
            (Some(a), Some(b)) => {
                if a.id != b.id || a.paused != b.paused { return true; }
                if a.paused { return a.pos_ms.abs_diff(b.pos_ms) > SEEK_SLACK_MS; }
                let expect = b.pos_ms as i64 + a.at_ms as i64 - b.at_ms as i64;
                (a.pos_ms as i64 - expect).unsigned_abs() > SEEK_SLACK_MS
            }
            _ => true,
        }
    }
}
/// How long a shared status lasts without a fresh one, and how often a playing sender renews it. The gap leaves
/// room for a missed refresh before friends stop believing the last one.
pub const LISTENING_TTL_SECS: u64 = 600;
pub const LISTENING_REFRESH_SECS: u64 = 300;
pub const MAX_LISTEN_LINE: usize = 160;
/// How long a paused Spotify song stays in the shared status.
pub const PAUSED_SHOW_SECS: u64 = 120;

/// A file that travels in chunks because it is far too large for one encrypted blob. The key and nonce in the
/// enclosing `File` are the file's key material; each chunk's key and nonce are derived from them and its index, and
/// every chunk is authenticated with the chunk index and plaintext length as additional data. `sha256` commits to the
/// whole file: see `crate::large::root_hash`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct LargeFile {
    /// The real size, which `File::size` is not (that one stays inside an old app's 25 MB limit).
    pub size: u64,
    pub chunks: u64,
    pub chunk_size: u64,
    /// Hex of the whole-file root hash.
    pub sha256: String,
    /// When the relay removes it, so the chat can say so.
    pub expires: u64,
}
impl LargeFile {
    pub fn chunk_count(size: u64, chunk_size: u64) -> u64 {
        if chunk_size == 0 { return 0; }
        size / chunk_size + u64::from(size % chunk_size != 0)
    }
    /// The length of one chunk's plaintext, given the file size.
    pub fn plain_len(&self, index: u64) -> Option<u64> {
        if index >= self.chunks { return None; }
        Some(if index + 1 == self.chunks { self.size - self.chunk_size * (self.chunks - 1) } else { self.chunk_size })
    }
    pub fn validate(&self, outer_size: u64) -> Result<(), ParseError> {
        if self.size <= commons_proto::LARGE_FILE_THRESHOLD { return bad("a large file must be above the large-file threshold"); }
        if self.chunk_size < commons_proto::MIN_LARGE_CHUNK_BYTES || self.chunk_size > commons_proto::MAX_LARGE_CHUNK_BYTES { return bad("bad large-file chunk size"); }
        if self.chunks == 0 || self.chunks > 1_000_000 || self.chunks != Self::chunk_count(self.size, self.chunk_size) { return bad("bad large-file chunk count"); }
        if !is_key(&self.sha256) || commons_proto::unhex32(&self.sha256).is_err() { return bad("bad large-file hash"); }
        if self.expires == 0 { return bad("bad large-file expiry"); }
        // Old apps check this one, so it has to stay plausible to them.
        if outer_size != self.size.min(commons_proto::MAX_BLOB_BYTES as u64) { return bad("bad large-file size"); }
        Ok(())
    }
}

/// What a voice message adds to its file: its length and a rough waveform (up to 64 peaks, one byte each, as hex).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Voice {
    pub ms: u32,
    #[serde(default)]
    pub peaks: String,
}
pub const MAX_VOICE_MS: u32 = 10 * 60 * 1000;
pub const MAX_VOICE_PEAKS: usize = 64;
impl Voice {
    pub fn validate(&self) -> Result<(), ParseError> {
        if self.ms == 0 || self.ms > MAX_VOICE_MS { return bad("bad voice length"); }
        if self.peaks.len() > MAX_VOICE_PEAKS * 2 || self.peaks.len() % 2 != 0 || hex::decode(&self.peaks).is_err() { return bad("bad voice waveform"); }
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "t")]
pub enum Envelope {
    Text { id: String, text: String, #[serde(default)] reply: Option<String>, ts: u64 },
    Edit { target: String, text: String, ts: u64 },
    Delete { target: String },
    React { target: String, emoji: String, on: bool },
    File {
        id: String, name: String, mime: String, size: u64, blob: String, key: String, nonce: String,
        #[serde(default)] caption: String,
        #[serde(default)] reply: Option<String>,
        ts: u64,
        /// Present only for a file too large for one blob. An app from 0.2.x parses this message as the ordinary
        /// `File` it already knows (serde ignores the field), so it gets no "unknown kind" notice: it just cannot
        /// download the bytes. `size` above then says `min(real size, MAX_BLOB_BYTES)` so that old validation passes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        large: Option<LargeFile>,
        /// A voice message: the file is a short audio clip, and this is what the player needs before it has any
        /// bytes (length and a picture of the sound). Optional and ignored by older apps, which show the clip as
        /// an ordinary audio attachment.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        voice: Option<Voice>,
    },
    Typing,
    /// A read receipt: "I have read this room up to the message `upto`" (a message id, never a position in someone's list). One more
    /// encrypted message in the room, so the relay learns what it learns from a typing hint (that a small message was sent then)
    /// and nothing about what was read. Sent only while the sender's "Send read receipts" is on, only when the room was looked at,
    /// and only forward (receipts.rs). An app from before 0.3.8 has no such kind: it parses it as `Unknown`, notes "newer version"
    /// in its log and goes on.
    Read { upto: String },
    /// Best-effort live data for a room (cursors, pointers, anything small and short-lived). Never history: not stored,
    /// not shown, not counted, not retried. `kind` is a short lowercase token, `data` is up to `MAX_LIVE` bytes of text the
    /// sender chose (nothing here is trusted), and `ts` lets a receiver drop what arrives long after it was meant.
    /// An older app has no such kind, parses it as `Unknown` and notes "newer version" once; nothing else happens.
    Live { kind: String, data: String, ts: u64 },
    /// "Come to this voice room's call": a ring. Sent inside the room's own MLS group, so the relay sees one more
    /// encrypted message and nothing else. `id` names one ring; `to` is one member's account (absent = everyone in the
    /// room who is not already in the call); `end` is the caller taking the ring back. Like `Live` it is for now, not
    /// history: never stored or shown, never retried, and a receiver ignores one that is stale (see `ring_check`).
    /// An older app has no such kind, parses it as `Unknown` and notes "newer version" once; nothing else happens.
    Ring {
        id: String,
        ts: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
        #[serde(default, skip_serializing_if = "is_false")]
        end: bool,
    },
    /// "Take this invite and call me": the start of a one-to-one call. Sent inside a room the two people already share, so the
    /// relay sees one more encrypted message. `to` is the one account it is for (everyone else in the room can read it, and
    /// can do nothing with it: the invite is for a two-person community whose id commits to exactly these two accounts, see
    /// `direct.rs`). `nonce` is the community's nonce, so the receiver can check the marker before it joins anything; `link` is
    /// an invite link to that community. Like `Ring` it is for now, not history: never stored or shown, never retried, and an
    /// older app has no such kind, parses it as `Unknown` and notes "newer version" once; nothing else happens.
    /// `sealed` (0.3.6 on) is the invite's secret key sealed to `to`'s account key (`direct::seal_for`); the `link` then carries a
    /// throwaway key in its place, so nobody else in the room can sign a join with it. An older receiver does not know the field and
    /// cannot use the link; the caller only seals for a peer whose profile says it can open it (`direct_sealed`).
    Direct { id: String, ts: u64, to: String, nonce: String, link: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sealed: Option<String> },
    /// A person's profile refresh. Extra fields ride on it rather than new kinds: an older Megatron parses this as the
    /// `Profile` it already knows and serde ignores the rest, so nothing new ever reaches an old app's `Unknown` notice.
    /// `listen` is the shared "listening to" status (`None` = nothing to share); `avatar`/`avatar_cleared` the picture.
    Profile {
        name: String,
        ts: u64,
        #[serde(default)]
        listen: Option<NowPlaying>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        avatar: Option<AvatarRef>,
        /// The picture was removed. A separate field because a missing field and a JSON `null` both become `None`.
        #[serde(default, skip_serializing_if = "is_false")]
        avatar_cleared: bool,
        /// This app can take part in one-to-one calls. Absent from older apps, which is how a caller can tell.
        #[serde(default, skip_serializing_if = "is_false")]
        direct: bool,
        /// This app can open a call offer whose invite secret is sealed to it (0.3.6 on). Absent from older apps.
        #[serde(default, skip_serializing_if = "is_false")]
        direct_sealed: bool,
    },
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

/// Where one profile picture is, and how to open it. The bytes themselves travel as a blob, encrypted the same way an
/// attachment's are, with this key and nonce; the relay only ever holds the ciphertext.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct AvatarRef {    pub blob: String,
    pub key: String,
    pub nonce: String,
    pub mime: String,
    pub size: u64,
    /// "circle" or "square": the frame the person chose. Old apps never see any of this object.
    pub shape: String,
}
impl AvatarRef {
    pub fn validate(&self) -> Result<(), ParseError> {
        if !is_id(&self.blob) || !is_key(&self.key) || self.nonce.len() != 24 || hex::decode(&self.nonce).is_err() { return bad("bad profile picture reference"); }
        if !valid_mime(&self.mime) || !self.mime.starts_with("image/") { return bad("a profile picture must be an image"); }
        if self.size == 0 || self.size > MAX_AVATAR { return bad("bad profile picture size"); }
        if self.shape != "circle" && self.shape != "square" { return bad("bad profile picture shape"); }
        Ok(())
    }
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
            File { id, name, mime, size, blob, key, nonce, caption, reply, large, voice, .. } => {
                if !is_id(id) || !is_id(blob) || !short(name, 255) || !valid_mime(mime) || *size > commons_proto::MAX_BLOB_BYTES as u64 || caption.len() > MAX_TEXT { return bad("bad file"); }
                if key.len() != 64 || nonce.len() != 24 || hex::decode(key).is_err() || hex::decode(nonce).is_err() { return bad("bad file key"); }
                if reply.as_deref().is_some_and(|r| !is_id(r)) { return bad("bad reply"); }
                if let Some(l) = large { l.validate(*size)?; }
                if let Some(v) = voice { if large.is_some() || !mime.starts_with("audio/") { return bad("bad voice message"); } v.validate()?; }
            }
            Typing | Leave | Unknown => {}
            Read { upto } => if !is_id(upto) { return bad("bad read receipt"); },
            Live { kind, data, .. } => if !valid_live_kind(kind) || data.len() > MAX_LIVE { return bad("bad live event"); },
            Direct { id, to, nonce, link, sealed, .. } => {
                if !is_id(id) || !is_key(to) || !is_id(nonce) || link.len() > 3000 || InviteLink::decode(link).is_err() { return bad("bad direct call"); }
                if sealed.as_ref().is_some_and(|s| s.len() > 256 || commons_proto::unb64(s).is_err()) { return bad("bad direct call"); }
            }
            Ring { id, to, .. } => if !is_id(id) || to.as_deref().is_some_and(|t| !is_key(t)) { return bad("bad ring"); },
            Profile { name, listen, avatar, avatar_cleared, .. } => {
                if name.is_empty() || !short(name, 64) { return bad("bad profile"); }
                if let Some(l) = listen { if l.title.is_empty() || !one_line(&l.title, MAX_LISTEN_LINE) || !one_line(&l.artist, MAX_LISTEN_LINE) || l.spotify.as_ref().is_some_and(|s| !s.valid()) { return bad("bad listening status"); } }
                if *avatar_cleared && avatar.is_some() { return bad("bad profile picture change"); }
                if let Some(a) = avatar { a.validate()?; }
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

/// How a room is used. Empty is an ordinary text room and is what an app from before voice rooms writes (and
/// reads); `"voice"` is a room whose call is the room itself. The field is additive on purpose: an older app's
/// `ChanMeta` ignores it, so a voice room simply appears there as a normal room and nothing reaches `Envelope`.
pub const ROOM_VOICE: &str = "voice";

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct ChanMeta {
    pub id: String,
    pub name: String,
    /// Empty for a text room. Kept as a small token, not an enum, so a kind from a newer app is carried and ignored
    /// (shown as a text room) rather than making the whole table unreadable.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
}
impl ChanMeta {
    fn valid_kind(k: &str) -> bool { k.len() <= 32 && k.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') }
}

/// Stored in the group-context extension of every group, so it is authenticated by MLS
/// and handed to a joining device inside the Welcome.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "k")]
pub enum GroupExt {
    /// The community control group: roster = its members; this is the roles/channels/invites table.
    /// `owner` is the founder, pinned in the community id. `transfers` is the signed chain from there to today's owner.
    Control { name: String, owner: String, nonce: String, admins: Vec<AdminGrant>, channels: Vec<ChanMeta>, invites: Vec<InviteMeta>, #[serde(default)] banned: Vec<String>, #[serde(default, skip_serializing_if = "Vec::is_empty")] transfers: Vec<OwnerStep>, #[serde(default, skip_serializing_if = "Vec::is_empty")] consents: Vec<Consent>,
        /// The owner-signed room colours. Additive: an older app ignores the field (and drops it if it rewrites the table, which
        /// the owner's device then puts back). It is only ever believed when the signature is the current owner's.
        #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "lenient_style")] style: Option<SignedStyle> },
    /// A channel group. `community` is the control group id; `name` is only a hint, the control table is authoritative.
    Channel { community: String, name: String, owner: String, nonce: String, admins: Vec<AdminGrant>, #[serde(default, skip_serializing_if = "Vec::is_empty")] transfers: Vec<OwnerStep> },
}

impl GroupExt {
    pub fn to_bytes(&self) -> Vec<u8> { serde_json::to_vec(self).unwrap_or_default() }
    pub fn parse(b: &[u8]) -> Result<GroupExt, ParseError> {
        if b.len() > 128 * 1024 { return bad("group ext too large"); }
        let g: GroupExt = serde_json::from_slice(b).map_err(|e| ParseError(format!("bad group ext: {e}")))?;
        match &g {
            GroupExt::Control { name, owner, nonce, admins, channels, invites, banned, transfers, consents, style } => {
                if style.as_ref().is_some_and(|s| s.doc.len() > MAX_STYLE_DOC || s.sig.len() > 200) { return bad("oversized style"); }
                if banned.len() > 2000 || transfers.len() > 200 { return bad("too many bans or transfers"); }
                if consents.len() > MAX_CONSENTS || consents.iter().any(|c| !is_key(&c.account) || c.sig.len() != 128) { return bad("bad consents"); }
                if name.is_empty() || !short(name, 64) || !is_key(owner) || nonce.len() != 32 || admins.len() > 512 || channels.len() > 500 || invites.len() > 200 { return bad("bad control ext"); }
                // A channel id goes into URLs and into markup: hex only, whoever wrote this table.
                for c in channels { if !is_gid(&c.id) || !short(&c.name, 64) || c.name.is_empty() || !ChanMeta::valid_kind(&c.kind) { return bad("bad channel"); } }
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

    /// What a 0.2.x app parses: the same variants, with the profile exactly as it was before pictures existed.
    #[derive(Deserialize, Debug, PartialEq)]
    #[serde(tag = "t")]
    enum OldEnvelope {
        Profile { name: String, ts: u64 },
        #[serde(other)]
        Unknown,
    }

    const AVATAR_JSON: &str = r#""avatar":{"blob":"00112233445566778899aabbccddeeff","key":"000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f","nonce":"000102030405060708090a0b","mime":"image/webp","size":1234,"shape":"circle"}"#;

    #[test]
    fn an_older_app_still_reads_a_profile_with_a_picture() {
        let new = format!(r#"{{"t":"Profile","name":"Ana","ts":5,{AVATAR_JSON}}}"#);
        // This version reads the picture...
        let e = Envelope::parse(new.as_bytes()).unwrap();
        assert!(matches!(e, Envelope::Profile { ref name, avatar: Some(ref a), avatar_cleared: false, .. } if name == "Ana" && a.size == 1234));
        // ...while a 0.2.x app reads the very same bytes as the Profile it already knew, not as an unknown kind.
        assert_eq!(serde_json::from_str::<OldEnvelope>(&new).unwrap(), OldEnvelope::Profile { name: "Ana".into(), ts: 5 });
        // The removal flag is also just an ignored field to it.
        let removed = br#"{"t":"Profile","name":"Ana","ts":6,"avatar_cleared":true}"#;
        assert_eq!(serde_json::from_slice::<OldEnvelope>(removed).unwrap(), OldEnvelope::Profile { name: "Ana".into(), ts: 6 });
        // With no picture, the bytes are exactly what 0.2.x sends and expects.
        let plain = Envelope::Profile { name: "Ana".into(), ts: 5, listen: None, avatar: None, avatar_cleared: false, direct: false, direct_sealed: false };
        // (listen is always written, as null when there is none: 0.2.x ignores it, and null clears a shown status)
        assert_eq!(plain.to_bytes(), br#"{"t":"Profile","name":"Ana","ts":5,"listen":null}"#);
    }

    /// What a 0.2.x app parses: the channel table it already knew, with no `kind` on a room.
    #[derive(Deserialize, Debug, PartialEq)]
    struct OldChanMeta { id: String, name: String }
    #[derive(Deserialize, Debug, PartialEq)]
    #[serde(tag = "k")]
    enum OldGroupExt {
        Control { name: String, owner: String, nonce: String, admins: Vec<AdminGrant>, channels: Vec<OldChanMeta>, invites: Vec<InviteMeta> },
        Channel { community: String, name: String, owner: String, nonce: String, admins: Vec<AdminGrant> },
    }

    #[test]
    fn a_voice_room_is_only_a_field_old_apps_ignore() {
        let (general, lobby) = ("ab".repeat(16), "cd".repeat(16));
        let ext = GroupExt::Control {
            name: "club".into(), owner: "e".repeat(64), nonce: "f".repeat(32), admins: vec![],
            channels: vec![
                ChanMeta { id: general.clone(), name: "general".into(), kind: String::new() },
                ChanMeta { id: lobby.clone(), name: "lobby".into(), kind: ROOM_VOICE.into() },
            ],
            invites: vec![], banned: vec![], transfers: vec![], consents: vec![], style: None,
        };
        let bytes = ext.to_bytes();
        // This version reads the kind back.
        assert_eq!(GroupExt::parse(&bytes).unwrap(), ext);
        // A 0.2.x app reads the very same bytes as two ordinary rooms: the extra field is ignored, not an error.
        let OldGroupExt::Control { channels, .. } = serde_json::from_slice::<OldGroupExt>(&bytes).unwrap() else { panic!("must read as a control table") };
        assert_eq!(channels, vec![
            OldChanMeta { id: general.clone(), name: "general".into() },
            OldChanMeta { id: lobby.clone(), name: "lobby".into() },
        ]);
        // The kind lives in group-context metadata, never in an `Envelope`: an old app syncing this table has no
        // message to parse, so nothing can reach its `Envelope::Unknown` notice.
        assert!(!String::from_utf8(bytes.clone()).unwrap().contains("\"t\":"));
        // A text room is still written exactly as 0.2.x wrote it: no empty field, no new spelling.
        let plain = ChanMeta { id: general.clone(), name: "general".into(), kind: String::new() };
        assert_eq!(serde_json::to_string(&plain).unwrap(), format!(r#"{{"id":"{id}","name":"general"}}"#, id = general));
        // A kind is a small lower-case token; a table that claims otherwise is refused.
        let mut bad = ext.clone();
        if let GroupExt::Control { channels, .. } = &mut bad { channels[1].kind = "Voice!".into(); }
        assert!(GroupExt::parse(&bad.to_bytes()).is_err());
        // A kind from a newer app is carried and shown as a text room here, rather than making the table unreadable.
        let mut later = ext;
        if let GroupExt::Control { channels, .. } = &mut later { channels[1].kind = "stage".into(); }
        assert_eq!(GroupExt::parse(&later.to_bytes()).unwrap(), later);
    }

    #[test]
    fn a_voice_message_is_an_audio_file_to_an_older_app() {
        let env = Envelope::File {
            id: "00112233445566778899aabbccddeeff".into(), name: "Voice message.webm".into(), mime: "audio/webm".into(), size: 900,
            blob: "ffeeddccbbaa99887766554433221100".into(), key: "00".repeat(32), nonce: "11".repeat(12), caption: String::new(), reply: None, ts: 5,
            large: None, voice: Some(Voice { ms: 2100, peaks: "0a1b2c".into() }),
        };
        let bytes = env.to_bytes();
        assert_eq!(Envelope::parse(&bytes).unwrap(), env);
        #[derive(Deserialize, Debug)]
        #[serde(tag = "t")]
        enum OldEnvelope {
            File { name: String, mime: String, ts: u64 },
            #[serde(other)]
            Unknown,
        }
        match serde_json::from_slice::<OldEnvelope>(&bytes).unwrap() {
            OldEnvelope::File { mime, .. } => assert_eq!(mime, "audio/webm"),
            OldEnvelope::Unknown => panic!("a voice message must read as a file"),
        }
        let bad = |f: &dyn Fn(&mut Voice), mime: &str| {
            let mut e = env.clone();
            if let Envelope::File { voice: Some(ref mut v), mime: ref mut m, .. } = e { f(v); *m = mime.into(); }
            assert!(Envelope::parse(&serde_json::to_vec(&e).unwrap()).is_err());
        };
        bad(&|v| v.ms = 0, "audio/webm");
        bad(&|v| v.peaks = "zz".into(), "audio/webm");
        bad(&|v| v.peaks = "00".repeat(65), "audio/webm");
        bad(&|_| {}, "image/png");
    }

    #[test]
    fn an_older_app_reads_a_large_file_as_an_ordinary_file() {
        let size = 28u64 << 30;
        let chunk_size = commons_proto::LARGE_CHUNK_BYTES;
        let large = LargeFile { size, chunks: LargeFile::chunk_count(size, chunk_size), chunk_size, sha256: "ab".repeat(32), expires: commons_proto::now_secs() + 7 * 86400 };
        let env = Envelope::File {
            id: "00112233445566778899aabbccddeeff".into(), name: "movie.mkv".into(), mime: "video/x-matroska".into(),
            size: size.min(commons_proto::MAX_BLOB_BYTES as u64), blob: "ffeeddccbbaa99887766554433221100".into(),
            key: "00".repeat(32), nonce: "11".repeat(12), caption: String::new(), reply: None, ts: 5, large: Some(large.clone()), voice: None,
        };
        let bytes = env.to_bytes();
        assert_eq!(Envelope::parse(&bytes).unwrap(), env);
        assert_ne!(Envelope::parse(&bytes).unwrap(), Envelope::Unknown);

        // What a 0.2.x app parses: no `large` field, and it checks that size is within its own blob limit.
        #[derive(Deserialize, Debug)]
        #[serde(tag = "t")]
        enum OldEnvelope {
            File { id: String, name: String, mime: String, size: u64, blob: String, key: String, nonce: String, #[serde(default)] caption: String, #[serde(default)] reply: Option<String>, ts: u64 },
            #[serde(other)]
            Unknown,
        }
        let old: OldEnvelope = serde_json::from_slice(&bytes).unwrap();
        match old {
            OldEnvelope::File { size: outer, name, .. } => {
                assert_eq!(name, "movie.mkv");
                assert!(outer <= commons_proto::MAX_BLOB_BYTES as u64, "an old app would refuse the message");
            }
            OldEnvelope::Unknown => panic!("a large file must never look like an unknown kind"),
        }

        // A small file is byte-for-byte what this version always sent: no empty extra field.
        let plain = Envelope::File { id: "00112233445566778899aabbccddeeff".into(), name: "a.txt".into(), mime: "text/plain".into(), size: 3, blob: "ffeeddccbbaa99887766554433221100".into(), key: "00".repeat(32), nonce: "11".repeat(12), caption: String::new(), reply: None, ts: 5, large: None, voice: None };
        assert!(!String::from_utf8(plain.to_bytes()).unwrap().contains("large"));

        // A mismatched outer size, chunk count or hash is refused by this version.
        let bad = |f: &dyn Fn(&mut LargeFile)| {
            let mut e = env.clone();
            let Envelope::File { large: Some(ref mut l), .. } = e else { unreachable!() };
            f(l);
            assert!(Envelope::parse(&serde_json::to_vec(&e).unwrap()).is_err());
        };
        bad(&|l| l.size += 1);
        bad(&|l| l.chunks += 1);
        bad(&|l| l.chunk_size = 1024);
        bad(&|l| l.sha256 = "zz".repeat(32));
        let mut small_outer = env.clone();
        if let Envelope::File { ref mut size, .. } = small_outer { *size = 1; }
        assert!(Envelope::parse(&serde_json::to_vec(&small_outer).unwrap()).is_err());
    }

    #[test]
    fn a_bad_picture_reference_is_refused() {
        let good = serde_json::json!({"blob":"00112233445566778899aabbccddeeff","key":"000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f","nonce":"000102030405060708090a0b","mime":"image/webp","size":1234,"shape":"circle"});
        let env = |a: serde_json::Value, cleared: bool| serde_json::to_vec(&serde_json::json!({"t":"Profile","name":"Ana","ts":5,"avatar":a,"avatar_cleared":cleared})).unwrap();
        // a picture that is not an image, names a blob badly, is huge, or claims both states
        let mut no_image = good.clone(); no_image["mime"] = "text/html".into();
        assert!(Envelope::parse(&env(no_image, false)).is_err());
        let mut bad_blob = good.clone(); bad_blob["blob"] = "../etc/passwd".into();
        assert!(Envelope::parse(&env(bad_blob, false)).is_err());
        let mut huge = good.clone(); huge["size"] = (MAX_AVATAR + 1).into();
        assert!(Envelope::parse(&env(huge, false)).is_err());
        let mut odd_shape = good.clone(); odd_shape["shape"] = "oval".into();
        assert!(Envelope::parse(&env(odd_shape, false)).is_err());
        assert!(Envelope::parse(&env(good.clone(), true)).is_err());
        assert!(Envelope::parse(&env(good, false)).is_ok());
    }
}

// ---------------------------------------------------------------------------------------------------------------- colours
/// Biggest style document an owner may publish (about 500 rooms at two colours each, with room to spare).
pub const MAX_STYLE_DOC: usize = 64 * 1024;
pub const MAX_STYLE_ROOMS: usize = 500;

/// `#rrggbb` and nothing else: the value ends up in CSS custom properties, so it must not be able to carry anything more.
pub fn valid_color(c: &str) -> bool { c.len() == 7 && c.starts_with('#') && c.bytes().skip(1).all(|b| b.is_ascii_hexdigit()) }

/// A room's (or the community card's) two colours: the main one and its highlight.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Duo { pub main: String, pub hi: String }
impl Duo {
    pub fn valid(&self) -> bool { valid_color(&self.main) && valid_color(&self.hi) }
    pub fn lower(&self) -> Duo { Duo { main: self.main.to_ascii_lowercase(), hi: self.hi.to_ascii_lowercase() } }
}

/// Most pattern numbers a look may name. The apps know only the first few (more can be added later); a number an app does not know
/// means "automatic" to it, so the bound is a small fixed range rather than the current count.
pub const MAX_LOOK_PATTERN: u8 = 31;
/// Backdrop strength in percent of the app's usual look (100 = unchanged, 0 = off).
pub const MAX_LOOK_DIM: u16 = 200;

/// How the owner wants one room (or the community card) to look beyond its colours. Every part is optional; a missing part is "automatic".
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct Look {
    /// The ASCII pattern on the card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<u8>,
    /// The ASCII pattern behind the chat (rooms only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat: Option<u8>,
    /// Backdrop strength behind the chat in percent (rooms only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dim: Option<u16>,
    /// Rooms only. When on, the Oracle bot stays out of the room on its own initiative (no chiming in, no surprise art, no wall drawing); a direct
    /// @mention is still answered. Only ever stored as `true`: unset means "as the room's own `@Oracle quiet` / `chime in` words say".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oracle_quiet: Option<bool>,
    /// Rooms only. When on, the Oracle bot may now and then doodle on this room's backdrop wall on its own. Off (unset) by default; only ever
    /// stored as `true`. A wall the owner locked is still left alone, and so is a room where Oracle stays quiet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oracle_draw: Option<bool>,
    /// Rooms only. The backdrop's own two colours (wash and ASCII) behind the chat. Unset: the backdrop uses the room's card colours. An app that
    /// does not know this field ignores it and keeps using the card colours, so the document stays valid and readable for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub back: Option<Duo>,
}
impl Look {
    pub fn is_empty(&self) -> bool { self.card.is_none() && self.chat.is_none() && self.dim.is_none() && self.oracle_quiet.is_none() && self.oracle_draw.is_none() && self.back.is_none() }
    pub fn valid(&self) -> bool {
        !self.is_empty() && self.back.as_ref().is_none_or(|d| d.valid()) && self.card.is_none_or(|n| n <= MAX_LOOK_PATTERN) && self.chat.is_none_or(|n| n <= MAX_LOOK_PATTERN) && self.dim.is_none_or(|n| n <= MAX_LOOK_DIM)
    }
}

/// Everything the owner has chosen. A room with no entry uses the app's own default colours.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct StyleDoc {
    /// Rises with every change the owner makes. A document with a lower number than the one already in force never wins,
    /// whoever carries it, so replaying an old table cannot undo a newer choice.
    pub ver: u64,
    pub ts: u64,
    #[serde(default)]
    pub community: Option<Duo>,
    #[serde(default)]
    pub rooms: std::collections::BTreeMap<String, Duo>,
    /// Rooms whose wall (the drawings on the room's backdrop) only the owner may draw on. Absent in older documents: nothing is locked.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub locked: std::collections::BTreeSet<String>,
    /// Per room, the wall's generation. The owner raising it is "clear": drawings and snapshots of an older generation are ignored by everyone.
    /// Absent in older documents: every wall is at generation 0.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub wall: std::collections::BTreeMap<String, u64>,
    /// Per room (or the community's own card, keyed by the community id), the owner's choice of patterns and backdrop strength. Absent in older
    /// documents, and an app that does not know it ignores it, so the signed text of a document without looks is byte for byte what it was.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub looks: std::collections::BTreeMap<String, Look>,
}
impl StyleDoc {
    pub fn validate(&self) -> Result<(), ParseError> {
        if self.ver == 0 || self.ver > (1 << 53) { return bad("bad style version"); }
        if self.community.as_ref().is_some_and(|d| !d.valid()) { return bad("bad community colours"); }
        if self.rooms.len() > MAX_STYLE_ROOMS || self.rooms.iter().any(|(g, d)| !is_gid(g) || !d.valid()) { return bad("bad room colours"); }
        if self.locked.len() > MAX_STYLE_ROOMS || self.locked.iter().any(|g| !is_gid(g)) { return bad("bad locked rooms"); }
        if self.wall.len() > MAX_STYLE_ROOMS || self.wall.iter().any(|(g, n)| !is_gid(g) || *n > (1 << 53)) { return bad("bad wall generations"); }
        if self.looks.len() > MAX_STYLE_ROOMS || self.looks.iter().any(|(g, l)| !is_gid(g) || !l.valid()) { return bad("bad looks"); }
        Ok(())
    }
}

/// The document as the exact JSON text that was signed, plus the owner's signature over it. Keeping the signed text
/// verbatim means no one has to agree on how to print JSON.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct SignedStyle { pub doc: String, pub sig: String }
fn lenient_style<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<SignedStyle>, D::Error> {
    // A malformed field is simply "no colours": the table around it stays readable, as it does for an app that has never heard of the field.
    let v = <Option<serde_json::Value> as Deserialize>::deserialize(d)?;
    Ok(v.and_then(|v| serde_json::from_value(v).ok()))
}
pub fn style_msg(community: &str, doc: &str) -> Vec<u8> {
    let mut m = b"commons/style/v1".to_vec();
    m.extend_from_slice(&commons_proto::sha256(&[community.as_bytes(), doc.as_bytes()]));
    m
}
pub fn sign_style(owner_key: &ed25519_dalek::SigningKey, community: &str, doc: &StyleDoc) -> Result<SignedStyle, ParseError> {
    use ed25519_dalek::Signer;
    doc.validate()?;
    let text = serde_json::to_string(doc).map_err(|e| ParseError(e.to_string()))?;
    if text.len() > MAX_STYLE_DOC { return bad("style too large"); }
    let sig = hex::encode(owner_key.sign(&style_msg(community, &text)).to_bytes());
    Ok(SignedStyle { doc: text, sig })
}
/// The document, if `owner` (hex account key) signed it for this community and every field is well-formed.
pub fn verify_style(owner: &str, community: &str, s: &SignedStyle) -> Result<StyleDoc, ParseError> {
    use ed25519_dalek::Verifier;
    if s.doc.len() > MAX_STYLE_DOC { return bad("style too large"); }
    let vk = commons_proto::unhex32(owner).ok().and_then(|a| ed25519_dalek::VerifyingKey::from_bytes(&a).ok()).ok_or(ParseError("bad owner key".into()))?;
    let sig = hex::decode(&s.sig).ok().and_then(|x| ed25519_dalek::Signature::from_slice(&x).ok()).ok_or(ParseError("bad signature".into()))?;
    vk.verify(&style_msg(community, &s.doc), &sig).map_err(|_| ParseError("colours not signed by the owner".into()))?;
    let doc: StyleDoc = serde_json::from_str(&s.doc).map_err(|e| ParseError(format!("bad style document: {e}")))?;
    doc.validate()?;
    Ok(doc)
}
