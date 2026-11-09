//! Persistent client state (serialized as JSON, then encrypted by the vault).
use crate::envelope::*;
use commons_proto::DeviceCert;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role { Member, Admin, Owner }
impl Role { pub fn name(&self) -> &'static str { match self { Role::Member => "member", Role::Admin => "admin", Role::Owner => "owner" } } }

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Kind { Control, Channel }

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(default)]
pub struct State {
    pub v: u32,
    pub device_secret: String,
    pub account_secret: String,
    pub cert: Option<DeviceCert>,
    pub name: String,
    pub relay: String,
    pub mls: String,
    pub registered: bool,
    pub communities: BTreeMap<String, Community>,
    pub groups: BTreeMap<String, GroupState>,
    pub chans: BTreeMap<String, ChannelLog>,
    pub profiles: BTreeMap<String, Profile>,
    /// What each account said it is listening to, when we heard it. Expires by itself (see `NowPlaying`'s TTL).
    #[serde(default)]
    pub listening: BTreeMap<String, Heard>,
    /// Share what rune plays with the people in our communities. Off unless the person turns it on.
    #[serde(default)]
    pub share_listening: bool,
    /// Where rune answers. Empty means the default.
    #[serde(default)]
    pub rune_url: String,
    /// What friends were last told, and when. A change is sent at once; a still-playing status is renewed now and then.
    #[serde(default)]
    pub listening_sent: Option<NowPlaying>,
    #[serde(default)]
    pub listening_sent_ts: u64,
    pub outbox: Vec<OutItem>,
    pub welcome_out: Vec<WelcomeOut>,
    pub acl_jobs: Vec<AclJob>,
    pub parked_welcomes: Vec<ParkedWelcome>,
    pub ack_welcomes: Vec<String>,
    pub own_ids: VecDeque<String>,
    pub joining: BTreeMap<String, InviteLink>,
    pub join_sent: BTreeSet<String>,
    pub verified: BTreeSet<String>,
    pub revoked: BTreeMap<String, String>,
    pub pairing: Option<Pairing>,
    pub notices: VecDeque<Notice>,
    pub profile_dirty: bool,
    /// The next profile message must say that the picture was removed, so that everyone lets go of it.
    #[serde(default)]
    pub avatar_removed: bool,
    pub lr_published: bool,
    pub typing_enabled: bool,
    /// The relay's invite secret, kept so that we can register again if the relay ever forgets this device.
    pub relay_invite: String,
    /// When this device last passed the relay's sign-up secret on, per community's control group.
    #[serde(default)]
    pub relay_secret_shared: BTreeMap<String, u64>,
    /// When this device last asked its other devices for the secret, per community's control group.
    #[serde(default)]
    pub relay_secret_asked: BTreeMap<String, u64>,
    /// When we last asked to join (community id -> time), and when we may next try.
    pub join_asked: BTreeMap<String, u64>,
    pub join_retry: BTreeMap<String, u64>,
    /// Joins that will not happen, and why, until the person dismisses them.
    pub join_refused: BTreeMap<String, JoinRefused>,
    /// The relay says this device is revoked: nothing more can happen here.
    pub self_revoked: bool,
    /// Devices of our own account we have seen, so that a new one can be pointed out.
    pub known_devices: BTreeSet<String>,
    /// The consent sent with each join still open (community id -> consent).
    pub join_consent: BTreeMap<String, Consent>,
    /// Signatures of consents this account took back, by leaving, being removed or cancelling a join. A community's
    /// table can go on holding one, so one of these never gets a Welcome accepted. Newest last.
    pub withdrawn: VecDeque<String>,
    /// Game keys of this device. They stay in this file: the relay never sees them, and pairing does not pass them on.
    /// Phones have no keyboard for a game to read, so the setting is not kept there.
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    pub game_keys: commons_gamekeys::Settings,
    /// Large uploads that have not finished yet, by the relay's file id. The relay is the source of truth for which
    /// chunks arrived; this holds the key material and enough to offer the same file again after a restart.
    #[serde(default)]
    pub large_uploads: BTreeMap<String, LargeUpload>,
    /// Large downloads, by message id: where the partial file is and how much of it has been written.
    #[serde(default)]
    pub large_downloads: BTreeMap<String, LargeDownload>,
    /// Where large downloads are saved. Empty means the system's Downloads folder.
    #[serde(default)]
    pub download_dir: String,
}

/// One large file on its way out. Chunk bytes are never stored here, only the key material and where we are.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct LargeUpload {
    pub gid: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
    /// The file's modification time when it was picked, so a changed file with the same name and size does not
    /// resume as if it were the same bytes.
    #[serde(default)]
    pub last_modified: u64,
    pub chunk_size: u64,
    pub chunks: u64,
    /// The relay's id for the file (also the message's `blob`).
    pub blob: String,
    pub key: String,
    pub nonce: String,
    pub caption: String,
    pub expires: u64,
    /// Chunk indexes the relay said it has, as of the last begin or the last chunk sent.
    #[serde(default)]
    pub uploaded: Vec<u64>,
    pub started: u64,
}

/// One large file coming in. The partial file lives on disk; `done` chunks have been decrypted, verified and
/// appended, and their plaintext hashes are kept so the whole-file check can be finished after a restart.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct LargeDownload {
    pub gid: String,
    pub msg_id: String,
    pub blob: String,
    pub name: String,
    pub save_name: String,
    pub mime: String,
    pub size: u64,
    pub chunks: u64,
    pub chunk_size: u64,
    pub sha256: String,
    pub key: String,
    pub nonce: String,
    pub expires: u64,
    pub dir: String,
    pub part: String,
    pub done: u64,
    #[serde(default)]
    pub hashes: Vec<String>,
    #[serde(default)]
    pub finished: bool,
    #[serde(default)]
    pub path: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct JoinRefused { pub name: String, pub reason: String, pub ts: u64 }

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct Profile {
    pub name: String,
    pub ts: u64,
    /// Where this account's picture is, if it has one. One picture per account, as there is one name per account.
    #[serde(default)]
    pub avatar: Option<AvatarRef>,
}

/// A status someone shared, and the local time it arrived: a sender that goes quiet (playback stops, the app
/// closes) stops being shown by itself.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Heard { pub what: NowPlaying, pub ts: u64 }

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Notice { pub ts: u64, pub kind: String, pub text: String }

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct Community {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub admins: Vec<String>,
    pub nonce: String,
    pub channels: Vec<ChanMeta>,
    pub invites: Vec<InviteMeta>,
    pub kick_queue: BTreeSet<String>,
    pub leaving: BTreeSet<String>,
    pub banned: Vec<String>,
    pub removed: bool,
    pub removed_reason: String,
    /// Secrets of invites this device created (the group only holds the public half).
    pub invite_sks: BTreeMap<String, String>,
    /// The signed chain of owners after the founder.
    #[serde(default)]
    pub transfers: Vec<commons_proto::OwnerStep>,
    /// Our own entry in the table, as last seen.
    #[serde(default)]
    pub my_consent: Option<Consent>,
    /// Consents waiting for an admin (us, if we are one) to put them in the table.
    #[serde(default)]
    pub consent_queue: BTreeMap<String, Consent>,
    /// The owner's colours in force, from the newest document whose signature was the owner's. Kept even when a table
    /// comes through without it (an older app rewrote the table), so the room does not flicker back to defaults.
    #[serde(default)]
    pub style: Option<StyleDoc>,
    #[serde(default)]
    pub style_sig: String,
    /// When this device (as owner) last put the colours back into the table on its own.
    #[serde(default)]
    pub style_repub: u64,
}
impl Community {
    /// Take a verified document if it is newer than the one in force: higher version, then later time, then the
    /// signature as the final tie-break, so every device settles on the same one whatever order it saw them in.
    pub fn adopt_style(&mut self, doc: StyleDoc, sig: &str) -> bool {
        let newer = match &self.style {
            None => true,
            Some(cur) => (doc.ver, doc.ts, sig) > (cur.ver, cur.ts, self.style_sig.as_str()),
        };
        if newer { self.style = Some(doc); self.style_sig = sig.to_string(); }
        newer
    }
    pub fn role_of(&self, acct: &str) -> Option<Role> {
        if acct == self.owner { Some(Role::Owner) } else if self.admins.iter().any(|a| a == acct) { Some(Role::Admin) } else { Some(Role::Member) }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PendingCommit {
    pub msg_id: String,
    pub data: String,
    pub welcome: Option<String>,
    pub recipients: Vec<String>,
    pub acl_remove: Vec<String>,
    pub posted: bool,
    pub desc: String,
    /// Accounts the relay must let in before this commit is posted.
    #[serde(default)]
    pub acl_add: Vec<String>,
    /// Queued requests this commit answers: dropped when it is applied, kept for another try if it loses.
    #[serde(default)]
    pub handled: Vec<u64>,
    /// Admin grants this commit takes away, to be withdrawn at the relay too.
    #[serde(default)]
    pub acl_demote: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ReqItem {
    pub seq: u64, pub tag: u8, pub data: String, pub tries: u32,
    /// A join request we cannot check against an invite: kept separately so that junk cannot crowd out real ones.
    #[serde(default)]
    pub unverified: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GroupState {
    pub gid: String,
    pub community: String,
    pub kind: Kind,
    pub cursor: u64,
    pub pending: Option<PendingCommit>,
    /// Our state may be older than something we sent: do not send before a fresh epoch.
    pub tainted: bool,
    /// We can no longer follow this group (lost our own commit): waiting to be re-added.
    pub broken: bool,
    pub broken_from: u64,
    pub join_epoch: u64,
    pub broken_count: u64,
    pub rejoin_sent: bool,
    pub rejoin_ts: u64,
    #[serde(default)]
    pub denied: bool,
    pub removed: bool,
    pub dev_seq: BTreeMap<String, u64>,
    pub accounts: BTreeSet<String>,
    pub requests: Vec<ReqItem>,
    pub gaps: u64,
    /// Rejoin requests already acted on (device + nonce), so a recorded request cannot be replayed to churn a device's leaf.
    #[serde(default)]
    pub rejoin_used: Vec<String>,
}
impl GroupState {
    pub fn new(gid: &str, community: &str, kind: Kind) -> Self {
        GroupState { gid: gid.into(), community: community.into(), kind, cursor: 0, pending: None, tainted: false, broken: false, broken_from: 0, join_epoch: 0, broken_count: 0,
            rejoin_sent: false, rejoin_ts: 0, denied: false, removed: false, dev_seq: BTreeMap::new(), accounts: BTreeSet::new(), requests: vec![], gaps: 0, rejoin_used: vec![] }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum MsgBody {
    Text(String),
    File {
        name: String, mime: String, size: u64, blob: String, key: String, nonce: String, caption: String,
        /// Too large for one blob: the real size, chunk layout and whole-file hash. Absent for ordinary files.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        large: Option<LargeFile>,
    },
    /// System line shown in the timeline (decrypt failures, rejoin notices...). Never silent.
    Notice(String),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Msg {
    pub id: String,
    pub sender: String,
    pub device: String,
    pub ts: u64,
    pub seq: u64,
    pub body: MsgBody,
    pub reply: Option<String>,
    pub edited: bool,
    pub deleted: bool,
    pub reactions: BTreeMap<String, BTreeSet<String>>,
    /// "sending", "sent", "received"
    pub status: String,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct ChannelLog {
    pub msgs: Vec<Msg>,
    pub orphans: Vec<(Envelope, String, u64)>,
    pub read_upto: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OutItem {
    pub id: String,
    pub gid: String,
    pub env: Envelope,
    /// Ciphertext once encrypted (persisted before posting so a crash never re-encrypts).
    pub cipher: Option<String>,
    pub tries: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WelcomeOut { pub id: String, pub device: String, pub data: String, pub gid: String }
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct AclJob {
    pub gid: String,
    #[serde(default)] pub add: Vec<String>,
    #[serde(default)] pub remove: Vec<String>,
    #[serde(default)] pub demote: Vec<String>,
    #[serde(default)] pub promote: Vec<String>,
    #[serde(default)] pub transfer: Option<commons_proto::OwnerStep>,
    #[serde(default)] pub after: u64,
    /// accounts cut off from call tokens (sent at once, with the sender's authority like any removal)
    #[serde(default)] pub call_remove: Vec<String>,
}
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ParkedWelcome { pub id: String, pub data: String, pub ts: u64 }

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Pairing { pub rid: String, pub secret: String, pub public: String }
