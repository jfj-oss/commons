//! The live channel: events for the people in a room right now (camera tiles, wall strokes), that the relay holds for
//! seconds in memory and never stores. See docs/THREAT-MODEL.md, "The live channel".
//!
//! These events do NOT go through the MLS application ratchet. A message that is not in the relay's log would still
//! advance the sender's ratchet, and a member who was offline for more than 2000 such steps could no longer decrypt the
//! next real message from that sender. Instead:
//!
//!  * Key. `MLS-Exporter("commons-live-v1", group id, 32)` of the group's current epoch. Every member derives the same
//!    key; nobody outside the group at that epoch can. Deriving it changes no MLS state and saves nothing.
//!  * Seal. ChaCha20-Poly1305 with a fresh random 96-bit nonce per event. The additional data binds the group id, the
//!    epoch and the sending device. The kind is not in the clear (the relay learns less); it is inside the ciphertext
//!    and under the signature, which is how it is bound.
//!  * Sender. The sender's device key (the one that signs its relay requests and its MLS leaf) signs, inside the
//!    ciphertext, group, epoch, device, nonce, kind, time and data. The receiver checks the signature against the
//!    device named by the relay and checks that this device is in the group's current roster, so one member cannot
//!    speak as another and a removed member is not heard. The nonce is under the signature, so a member cannot
//!    re-seal another's signed event under a new nonce to get it past the replay filter.
//!  * Epochs. The current epoch and the one before are accepted (a commit is not seen by everybody at the same
//!    moment); anything else is dropped. A event from an epoch we have not reached yet is held a few seconds.
//!  * Replay. A nonce is accepted once; events older than `LIVE_MAX_AGE` (by the signed time) are dropped.
use crate::client::*;
use crate::envelope::*;
use crate::mls;
use crate::state::*;
use crate::transport::*;
use anyhow::Result;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use commons_proto::*;
use ed25519_dalek::{Signature, Signer as _, SigningKey, Verifier as _, VerifyingKey};
use openmls_traits::OpenMlsProvider;
use rand::RngCore;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

pub const LABEL: &str = "commons-live-v1";
const DOM: &[u8] = b"commons/live/v1\0";
const VER: u8 = 1;
/// Plaintexts are padded to a multiple of this, so a size says little about what was sent.
const PAD: usize = 128;
/// Most live events sent per room per second on the unstored channel (the old, logged path keeps its four).
pub const LIVE_CHAN_PER_SEC: usize = 10;
/// Most events a receiver takes from one sender in a room per second from this channel.
pub const LIVE_CHAN_TAKE_PER_SEC: u32 = 20;
/// How long a relay that does not know the channel is left alone before asking again.
pub const LIVE_RECHECK_SECS: u64 = 120;
/// Events that cannot be used yet (sealed under an epoch we have not reached, or depending on log entries we have not read):
/// how many are held in all and per sender, and for how many seconds.
const HOLD_MAX: usize = 64;
const HOLD_PER_SENDER: usize = 16;
const HOLD_SECS: u64 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveErr {
    Malformed,
    /// Sealed under an epoch newer than ours: wait for our own commit to arrive.
    Future,
    /// Under an epoch we no longer (or never) had.
    Epoch,
    /// The device is not in the group's current roster.
    Roster,
    Auth,
    Signature,
    Stale,
    Replay,
    Oversized,
}

/// The key of one epoch.
#[derive(Clone)]
pub struct EpochKey { pub epoch: u64, pub key: [u8; 32] }
/// What one room's live traffic needs: the keys of this epoch and the one before, and who is in the room (device to account).
#[derive(Clone, Default)]
pub struct RoomKeys {
    pub cur: Option<EpochKey>,
    pub prev: Option<EpochKey>,
    pub roster: BTreeMap<String, String>,
    pub(crate) checked_ms: u128,
}
impl RoomKeys {
    /// Move to `epoch` (with its key and roster). The epoch we were at becomes "previous" only when this is the very next one.
    pub fn advance(&mut self, epoch: u64, key: [u8; 32], roster: BTreeMap<String, String>) {
        match self.cur.take() {
            Some(c) if c.epoch == epoch => { self.cur = Some(c); self.roster = roster; return; }
            Some(c) if c.epoch + 1 == epoch => self.prev = Some(c),
            _ => self.prev = None,
        }
        self.cur = Some(EpochKey { epoch, key });
        self.roster = roster;
    }
    fn key_for(&self, epoch: u64) -> Result<&[u8; 32], LiveErr> {
        match (&self.cur, &self.prev) {
            (Some(c), _) if c.epoch == epoch => Ok(&c.key),
            (_, Some(p)) if p.epoch == epoch => Ok(&p.key),
            (Some(c), _) if epoch > c.epoch => Err(LiveErr::Future),
            _ => Err(LiveErr::Epoch),
        }
    }
}

/// Nonces already accepted, so the same sealed event is never taken twice.
#[derive(Default)]
pub struct Seen { m: HashMap<[u8; 12], u64> }
impl Seen {
    /// True the first time a nonce is offered.
    pub fn first(&mut self, nonce: [u8; 12], now: u64) -> bool {
        if self.m.len() > 16_384 { self.m.retain(|_, t| *t + 130 >= now); if self.m.len() > 16_384 { self.m.clear(); } }
        self.m.insert(nonce, now).is_none()
    }
    pub fn len(&self) -> usize { self.m.len() }
    pub fn is_empty(&self) -> bool { self.m.is_empty() }
}

/// A live event that passed every check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened { pub account: String, pub device: String, pub kind: String, pub data: String, pub ts: u64, /// How far the sender had read the community's signed table log (its control group) when it sent this. A receiver
    /// that has not read that far yet holds the event: it may depend on what is in those entries (a wall clear or lock).
    pub dep: u64 }

fn aad(gid: &str, epoch: u64, device: &[u8; 32]) -> Vec<u8> {
    let mut a = DOM.to_vec();
    a.extend((gid.len() as u16).to_be_bytes()); a.extend(gid.as_bytes());
    a.extend(epoch.to_be_bytes()); a.extend(device);
    a
}
fn signed_part(gid: &str, epoch: u64, device: &[u8; 32], nonce: &[u8; 12], kind: &str, ts: u64, dep: u64, data: &str) -> Vec<u8> {
    let mut m = aad(gid, epoch, device);
    m.extend(nonce);
    m.push(kind.len() as u8); m.extend(kind.as_bytes());
    m.extend(ts.to_be_bytes()); m.extend(dep.to_be_bytes());
    m.extend((data.len() as u16).to_be_bytes()); m.extend(data.as_bytes());
    m
}

/// Seal one event. `signer` is this device's key; its public half is the device id.
pub fn seal(key: &[u8; 32], gid: &str, epoch: u64, signer: &SigningKey, kind: &str, data: &str, ts: u64, dep: u64) -> Result<Vec<u8>, LiveErr> {
    seal_as(key, gid, epoch, &signer.verifying_key().to_bytes(), signer, kind, data, ts, dep)
}
/// `seal` naming a device other than the signer's own: only a hostile sender does that (the tests do, to see it fail).
pub fn seal_as(key: &[u8; 32], gid: &str, epoch: u64, device: &[u8; 32], signer: &SigningKey, kind: &str, data: &str, ts: u64, dep: u64) -> Result<Vec<u8>, LiveErr> {
    if !valid_live_kind(kind) || data.len() > MAX_LIVE { return Err(LiveErr::Oversized); }
    let device = *device;
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let sig = signer.sign(&signed_part(gid, epoch, &device, &nonce, kind, ts, dep, data));
    let mut pt = sig.to_bytes().to_vec();
    pt.push(kind.len() as u8); pt.extend(kind.as_bytes());
    pt.extend(ts.to_be_bytes()); pt.extend(dep.to_be_bytes());
    pt.extend((data.len() as u16).to_be_bytes()); pt.extend(data.as_bytes());
    while pt.len() % PAD != 0 { pt.push(0); }
    let ct = ChaCha20Poly1305::new(key.into()).encrypt(Nonce::from_slice(&nonce), Payload { msg: &pt, aad: &aad(gid, epoch, &device) }).map_err(|_| LiveErr::Malformed)?;
    let mut out = vec![VER];
    out.extend(epoch.to_be_bytes()); out.extend(nonce); out.extend(ct);
    Ok(out)
}

/// Open one sealed event that the relay says came from device `from`. Checks, in this order: shape, epoch, the sender
/// being in the roster, the seal, the signature, the time, and that the nonce is new.
pub fn open(keys: &RoomKeys, seen: &mut Seen, gid: &str, from: &str, blob: &[u8], now: u64) -> Result<Opened, LiveErr> {
    if blob.len() < 1 + 8 + 12 + 16 || blob.len() > commons_proto::LIVE_MAX_BLOB || blob[0] != VER { return Err(LiveErr::Malformed); }
    let epoch = u64::from_be_bytes(blob[1..9].try_into().unwrap());
    let nonce: [u8; 12] = blob[9..21].try_into().unwrap();
    let key = keys.key_for(epoch)?;
    let account = keys.roster.get(from).ok_or(LiveErr::Roster)?;
    let device = unhex32(from).map_err(|_| LiveErr::Malformed)?;
    let pt = ChaCha20Poly1305::new(key.into()).decrypt(Nonce::from_slice(&nonce), Payload { msg: &blob[21..], aad: &aad(gid, epoch, &device) }).map_err(|_| LiveErr::Auth)?;
    // sig(64) | kind len(1) | kind | ts(8) | dep(8) | data len(2) | data | zero padding
    if pt.len() < 64 + 1 { return Err(LiveErr::Malformed); }
    let sig = Signature::from_slice(&pt[..64]).map_err(|_| LiveErr::Malformed)?;
    let kl = pt[64] as usize;
    let mut p = 65;
    let take = |p: &mut usize, n: usize| -> Result<&[u8], LiveErr> { if *p + n > pt.len() { Err(LiveErr::Malformed) } else { let s = &pt[*p..*p + n]; *p += n; Ok(s) } };
    let kind = std::str::from_utf8(take(&mut p, kl)?).map_err(|_| LiveErr::Malformed)?.to_string();
    let ts = u64::from_be_bytes(take(&mut p, 8)?.try_into().unwrap());
    let dep = u64::from_be_bytes(take(&mut p, 8)?.try_into().unwrap());
    let dl = u16::from_be_bytes(take(&mut p, 2)?.try_into().unwrap()) as usize;
    if dl > MAX_LIVE { return Err(LiveErr::Oversized); }
    let data = std::str::from_utf8(take(&mut p, dl)?).map_err(|_| LiveErr::Malformed)?.to_string();
    if !valid_live_kind(&kind) || pt[p..].iter().any(|b| *b != 0) { return Err(LiveErr::Malformed); }
    let vk = VerifyingKey::from_bytes(&device).map_err(|_| LiveErr::Malformed)?;
    vk.verify(&signed_part(gid, epoch, &device, &nonce, &kind, ts, dep, &data), &sig).map_err(|_| LiveErr::Signature)?;
    if ts + LIVE_MAX_AGE < now || ts > now + LIVE_MAX_AHEAD { return Err(LiveErr::Stale); }
    if !seen.first(nonce, now) { return Err(LiveErr::Replay); }
    Ok(Opened { account: account.clone(), device: from.to_string(), kind, data, ts, dep })
}

/// Whether the relay is known to carry the channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Support { #[default] Unknown, Yes,
    /// Not supported; ask again at this time (seconds).
    No(u64), }

/// Everything the client keeps for the channel. None of it is saved.
#[derive(Default)]
pub struct LiveChan {
    pub support: Support,
    boot: String,
    cursors: BTreeMap<String, u64>,
    keys: BTreeMap<String, RoomKeys>,
    seen: Seen,
    wire: VecDeque<(String, Vec<u8>, u128)>,
    held: Vec<Held>,
    /// Set when the app runs its own sender and poller threads: the ordinary round then only sends.
    pub background: bool,
    /// The rooms the last poll asked about, and when that list last changed.
    polled_rooms: Vec<String>,
    rooms_changed: u64,
    /// The rooms of the poll the app's poller thread is waiting on now: a room not in it is polled by the ordinary round.
    pub(crate) polling: BTreeSet<String>,
    /// Test builds only: behave like a version from before the channel (send through the log, never poll).
    pub legacy: bool,
    /// What the channel did, for tests and the debug view.
    pub stats: LiveStats,
}
#[derive(Default, Clone, Debug)]
pub struct LiveStats { pub sealed: u64, pub taken: u64, pub dropped: u64, pub posts: u64, pub polls: u64 }

fn now_ms() -> u128 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0) }

enum Held {
    Raw { gid: String, from: String, blob: Vec<u8>, at: u64 },
    Ready { gid: String, o: Opened, at: u64 },
}
impl Held {
    fn at(&self) -> u64 { match self { Held::Raw { at, .. } | Held::Ready { at, .. } => *at } }
    fn device(&self) -> &str { match self { Held::Raw { from, .. } => from, Held::Ready { o, .. } => &o.device } }
}

/// One round of work for the channel, planned with the client held and run without it.
pub struct LivePlan {
    api: Api,
    key: SigningKey,
    sends: Vec<(String, Vec<Vec<u8>>)>,
    poll: Option<LivePollReq>,
    quick: bool,
}
#[derive(Default)]
pub struct LiveDone {
    /// Learned from an answer: the relay does (true) or does not (false) carry the channel.
    support: Option<bool>,
    poll: Option<LivePollResp>,
    posts: u64,
    polls: u64,
}

fn is_gone(e: &anyhow::Error) -> bool { relay_status(e) == Some(404) }

impl LivePlan {
    pub fn wants_poll(&self) -> bool { self.poll.is_some() }
    /// The rooms the poll asks about.
    pub fn rooms(&self) -> Vec<String> { self.poll.as_ref().map(|p| p.cursors.keys().cloned().collect()).unwrap_or_default() }
    /// The set of rooms changed lately: a poll already in flight would not cover a new room, so keep polls short for a while.
    pub fn quick(&self) -> bool { self.quick }
    /// The network part. `wait_ms` is how long the relay may hold the poll for news.
    pub fn run(self, wait_ms: u64) -> LiveDone {
        let mut d = LiveDone::default();
        for (gid, blobs) in &self.sends {
            for chunk in blobs.chunks(LIVE_POST_MAX) {
                match self.api.live_post(&self.key, gid, chunk) {
                    Ok(_) => { d.support = Some(true); d.posts += 1; }
                    Err(e) if is_gone(&e) => { d.support = Some(false); return d; }
                    Err(e) if !e.is::<RelayError>() => return d, // the network is down: sync will say so
                    Err(e) => log::debug!("live post: {e}"),      // limited or refused: live data is never retried
                }
            }
        }
        if let Some(mut req) = self.poll {
            req.wait_ms = wait_ms;
            d.polls += 1;
            match self.api.live_poll(&self.key, &req) {
                Ok(r) => { d.support = Some(true); d.poll = Some(r); }
                Err(e) if is_gone(&e) => d.support = Some(false),
                Err(e) => log::debug!("live poll: {e}"),
            }
        }
        d
    }
}
impl LiveDone {
    pub fn failed(&self) -> bool { self.poll.is_none() && self.support != Some(true) }
    pub fn unsupported(&self) -> bool { self.support == Some(false) }
}

impl Client {
    /// Whether to send live data over the unstored channel (anything but a relay known not to carry it).
    pub(crate) fn live_chan_on(&self) -> bool { !self.live.legacy && !matches!(self.live.support, Support::No(_)) }
    pub fn live_support(&self) -> Support { self.live.support }
    pub fn live_stats(&self) -> LiveStats { self.live.stats.clone() }
    pub fn set_live_background(&mut self, on: bool) { self.live.background = on; }
    /// The app's poller says which rooms its request in flight covers (none between requests, or when it has no plan).
    pub fn set_live_polling(&mut self, rooms: Vec<String>) { self.live.polling = rooms.into_iter().collect(); }
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn debug_live_legacy(&mut self, on: bool) { self.live.legacy = on; }
    /// Pretend the wait before asking a relay again is over.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn debug_live_recheck_now(&mut self) { if let Support::No(_) = self.live.support { self.live.support = Support::No(0); } }
    /// All of this device's MLS state, as bytes: a test compares it before and after live traffic.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn debug_mls_dump(&self) -> Vec<u8> { self.prov.dump() }
    /// Seal `n` events at once, past the send-rate limit, and post them: a member who sends far more than the app would.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn debug_live_burst(&mut self, gid: &str, n: usize) -> usize {
        let mut sealed = 0;
        for chunk in 0..n.div_ceil(50) {
            for i in 0..50.min(n - chunk * 50) { if self.live_chan_seal(gid, "burst", &i.to_string()) { sealed += 1; } }
            if let Some(p) = self.live_plan_with(true, false) { let d = p.run(0); self.live_finish(d); }
        }
        sealed
    }

    /// Can this room seal live events now? The group is ours and in working order. Unlike a message it needs no
    /// fresh state of its own; it only needs the key of the epoch we are at.
    pub(crate) fn live_chan_ready(&self, gid: &str) -> bool {
        self.st.groups.get(gid).is_some_and(|g| !g.removed && !g.broken && !g.tainted) && self.synced.contains(gid)
    }

    /// Bring a room's keys and roster up to the group's epoch. Reads the MLS group, changes nothing in it.
    fn live_refresh(&mut self, gid: &str) -> bool {
        let now = now_ms();
        if self.live.keys.get(gid).is_some_and(|k| k.cur.is_some() && now < k.checked_ms + 250) { return true; }
        let Some(g) = self.load_group(gid) else { return false };
        if !g.is_active() { self.live.keys.remove(gid); return false; }
        let epoch = g.epoch().as_u64();
        let have = self.live.keys.get(gid).and_then(|k| k.cur.as_ref()).map(|c| c.epoch);
        if have != Some(epoch) {
            let Ok(k) = g.export_secret(self.prov.crypto(), LABEL, gid.as_bytes(), 32) else { return false };
            let Ok(k) = <[u8; 32]>::try_from(k) else { return false };
            let roster: BTreeMap<String, String> = mls::members_of(&g).into_iter().map(|(_, c)| (c.device, c.account)).collect();
            self.live.keys.entry(gid.to_string()).or_default().advance(epoch, k, roster);
        }
        if let Some(k) = self.live.keys.get_mut(gid) { k.checked_ms = now; }
        true
    }

    /// Seal an event for the channel and queue it for the next post. None when the room cannot do it now.
    pub(crate) fn live_chan_seal(&mut self, gid: &str, kind: &str, data: &str) -> bool {
        if !self.live_chan_ready(gid) || !self.live_refresh(gid) { return false; }
        let Some(c) = self.live.keys.get(gid).and_then(|k| k.cur.clone()) else { return false };
        let Ok(dk) = self.dev_key() else { return false };
        match seal(&c.key, gid, c.epoch, &dk, kind, data, now_secs(), self.live_dep(gid)) {
            Ok(b) => {
                self.live.wire.push_back((gid.to_string(), b, now_ms()));
                while self.live.wire.len() > 64 { self.live.wire.pop_front(); }
                self.live.stats.sealed += 1;
                true
            }
            Err(_) => false,
        }
    }

    /// Plan the channel's network work: post what is waiting and, when asked, poll. `poll` is false when the app's own
    /// poller thread does that. None when there is nothing to do.
    pub fn live_plan(&mut self, poll: bool) -> Option<LivePlan> { self.live_plan_with(true, poll) }
    /// As `live_plan`, choosing which halves to plan: the app's sender thread posts only, its poller thread polls only.
    pub fn live_plan_with(&mut self, send: bool, poll: bool) -> Option<LivePlan> {
        if !self.has_identity() || self.st.self_revoked || self.live.legacy { return None; }
        let now_m = now_ms();
        let mut sends: BTreeMap<String, Vec<Vec<u8>>> = BTreeMap::new();
        while send {
            let Some((gid, b, at)) = self.live.wire.pop_front() else { break };
            if now_m < at + 3000 && self.live_chan_on() { sends.entry(gid).or_default().push(b); }
        }
        let poll = poll.then(|| self.live_poll_req()).flatten();
        if sends.is_empty() && poll.is_none() { return None; }
        let key = self.dev_key().ok()?;
        let mut quick = false;
        if let Some(r) = &poll {
            let rooms: Vec<String> = r.cursors.keys().cloned().collect();
            if rooms != self.live.polled_rooms { self.live.polled_rooms = rooms; self.live.rooms_changed = now_secs(); }
            quick = now_secs() < self.live.rooms_changed + 10;
        }
        Some(LivePlan { api: self.api.clone(), key, sends: sends.into_iter().collect(), poll, quick })
    }

    /// The poll to make now: none if the relay is known not to carry the channel and it is not yet time to ask again.
    pub fn live_poll_req(&self) -> Option<LivePollReq> {
        if let Support::No(until) = self.live.support { if now_secs() < until { return None; } }
        // Only rooms we can read. A room we have not polled before starts at "now" (u64::MAX: the relay answers at once with where the room stands), not at what is buffered.
        let cursors: BTreeMap<String, u64> = self.st.groups.values()
            .filter(|g| g.kind == Kind::Channel && !g.removed && !g.broken)
            .map(|g| (g.gid.clone(), self.live.cursors.get(&g.gid).copied().unwrap_or(u64::MAX))).collect();
        if cursors.is_empty() { return None; }
        Some(LivePollReq { boot: self.live.boot.clone(), cursors, wait_ms: 0 })
    }

    /// How far this device has read the control log (the community's signed table) of the community a room belongs to.
    fn live_control_cursor(&self, gid: &str) -> u64 {
        let Some(g) = self.st.groups.get(gid) else { return 0 };
        self.st.groups.values().find(|x| x.kind == Kind::Control && x.community == g.community && !x.removed).map_or(0, |x| x.cursor)
    }
    /// What goes into a sent event as its dependency: our own place in that log, one further when a change of ours is
    /// still on its way into it (that change will be at least one entry later).
    fn live_dep(&self, gid: &str) -> u64 {
        let Some(g) = self.st.groups.get(gid) else { return 0 };
        let pending = self.st.groups.values().any(|x| x.kind == Kind::Control && x.community == g.community && !x.removed && x.pending.is_some());
        self.live_control_cursor(gid) + pending as u64
    }

    /// Apply what the network part found. Local work only; nothing here is saved.
    pub fn live_finish(&mut self, d: LiveDone) {
        self.live.stats.posts += d.posts;
        self.live.stats.polls += d.polls;
        match d.support {
            Some(true) => self.live.support = Support::Yes,
            Some(false) => { self.live.support = Support::No(now_secs() + LIVE_RECHECK_SECS); self.live.wire.clear(); self.live.held.clear(); }
            None => {}
        }
        let now = now_secs();
        let mut work: Vec<Held> = std::mem::take(&mut self.live.held).into_iter().filter(|h| h.at() + HOLD_SECS >= now).collect();
        if let Some(r) = d.poll {
            if r.boot != self.live.boot { self.live.boot = r.boot.clone(); self.live.cursors.clear(); }
            for (gid, g) in r.groups {
                if !self.st.groups.get(&gid).is_some_and(|s| s.kind == Kind::Channel && !s.removed) { continue; }
                // Two polls can be out at once (the poller's long one and a round's short one for a new room): the older
                // answer must not move a room back, or its events would come twice (and be dropped as replays).
                let at = self.live.cursors.entry(gid.clone()).or_insert(0);
                *at = (*at).max(g.next);
                for it in g.events {
                    if let Ok(b) = unb64(&it.data) { work.push(Held::Raw { gid: gid.clone(), from: it.from, blob: b, at: now }); }
                }
            }
        }
        self.live_process(work);
    }

    /// Give held events another look: called when the control log has moved on (after a sync).
    pub(crate) fn live_retry_held(&mut self) {
        if self.live.held.is_empty() { return; }
        let now = now_secs();
        let work: Vec<Held> = std::mem::take(&mut self.live.held).into_iter().filter(|h| h.at() + HOLD_SECS >= now).collect();
        self.live_process(work);
    }

    fn live_hold(&mut self, h: Held) {
        let dev = h.device().to_string();
        if self.live.held.len() >= HOLD_MAX || self.live.held.iter().filter(|x| x.device() == dev).count() >= HOLD_PER_SENDER { self.live.stats.dropped += 1; } else { self.live.held.push(h); }
    }

    fn live_process(&mut self, work: Vec<Held>) {
        if work.is_empty() { return; }
        let me = self.my_device();
        let now = now_secs();
        let mut fresh: BTreeMap<String, bool> = BTreeMap::new();
        for h in work {
            match h {
                Held::Raw { gid, from, blob, at } => {
                    if from == me { continue; }
                    let ok = *fresh.entry(gid.clone()).or_insert_with(|| self.live_refresh(&gid));
                    if !ok { self.live.stats.dropped += 1; continue; }
                    let r = match self.live.keys.get(&gid) { Some(k) => open(k, &mut self.live.seen, &gid, &from, &blob, now), None => Err(LiveErr::Epoch) };
                    match r {
                        Ok(o) => self.live_deliver(gid, o, at),
                        // Sealed under an epoch we have not reached: our own copy of the commit is on its way.
                        Err(LiveErr::Future) => self.live_hold(Held::Raw { gid, from, blob, at }),
                        Err(_) => self.live.stats.dropped += 1,
                    }
                }
                Held::Ready { gid, o, at } => self.live_deliver(gid, o, at),
            }
        }
    }

    /// Hand a checked event to the rest of the client, unless it depends on log entries we have not read yet: then it waits.
    fn live_deliver(&mut self, gid: String, o: Opened, at: u64) {
        if o.dep > self.live_control_cursor(&gid) { self.live_hold(Held::Ready { gid, o, at }); return; }
        self.live.stats.taken += 1;
        self.take_live_limited(&gid, &o.account, o.kind, o.data, o.ts, LIVE_CHAN_TAKE_PER_SEC);
    }

    /// Plan, run and apply the channel once, holding nothing in between that matters. For tests and for the
    /// synchronous `tick`: the app's threads do the same in two halves.
    pub fn live_tick(&mut self) {
        self.flush_live();
        if let Some(p) = self.live_plan(true) { let d = p.run(0); self.live_finish(d); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    const GID: &str = "aabbccddeeff00112233445566778899";
    struct W { a: SigningKey, b: SigningKey, keys: RoomKeys, seen: Seen }
    fn w() -> W {
        let (a, b) = (SigningKey::generate(&mut OsRng), SigningKey::generate(&mut OsRng));
        let mut keys = RoomKeys::default();
        keys.advance(4, [4; 32], BTreeMap::new());
        let roster = BTreeMap::from([(hex::encode(a.verifying_key().to_bytes()), "alice".to_string()), (hex::encode(b.verifying_key().to_bytes()), "bob".to_string())]);
        keys.advance(5, [5; 32], roster);
        W { a, b, keys, seen: Seen::default() }
    }
    fn dev(k: &SigningKey) -> String { hex::encode(k.verifying_key().to_bytes()) }
    fn now() -> u64 { now_secs() }

    #[test]
    fn an_event_round_trips_and_carries_who_sent_it() {
        let mut w = w();
        let blob = seal(&[5; 32], GID, 5, &w.a, "tile", "{\"x\":1}", now(), 0).unwrap();
        let o = open(&w.keys, &mut w.seen, GID, &dev(&w.a), &blob, now()).unwrap();
        assert_eq!((o.account.as_str(), o.kind.as_str(), o.data.as_str()), ("alice", "tile", "{\"x\":1}"));
        assert_eq!(blob.len() % 1, 0);
        assert!((blob.len() - 21 - 16) % PAD == 0, "padded to {PAD}");
        // The kind and data are not in the clear.
        assert!(!blob.windows(4).any(|x| x == b"tile"));
    }

    #[test]
    fn the_epoch_must_be_the_current_or_the_one_before() {
        let mut w = w();
        let a = dev(&w.a);
        // the previous epoch's key is still good
        let old = seal(&[4; 32], GID, 4, &w.a, "k", "x", now(), 0).unwrap();
        assert!(open(&w.keys, &mut w.seen, GID, &a, &old, now()).is_ok());
        // two back is not, and nor is one we have not reached (that one is told apart so it can wait)
        let older = seal(&[3; 32], GID, 3, &w.a, "k", "x", now(), 0).unwrap();
        assert_eq!(open(&w.keys, &mut w.seen, GID, &a, &older, now()), Err(LiveErr::Epoch));
        let newer = seal(&[6; 32], GID, 6, &w.a, "k", "x", now(), 0).unwrap();
        assert_eq!(open(&w.keys, &mut w.seen, GID, &a, &newer, now()), Err(LiveErr::Future));
        // right epoch number, wrong key (someone not in the group guessing)
        let wrong_key = seal(&[9; 32], GID, 5, &w.a, "k", "x", now(), 0).unwrap();
        assert_eq!(open(&w.keys, &mut w.seen, GID, &a, &wrong_key, now()), Err(LiveErr::Auth));
        // a key that is the current one but claimed for the previous epoch number
        let mislabelled = seal(&[5; 32], GID, 4, &w.a, "k", "x", now(), 0).unwrap();
        assert_eq!(open(&w.keys, &mut w.seen, GID, &a, &mislabelled, now()), Err(LiveErr::Auth));
        // after a jump of more than one epoch there is no "previous"
        let mut k = w.keys.clone();
        k.advance(9, [9; 32], k.roster.clone());
        assert!(k.prev.is_none());
    }

    #[test]
    fn an_event_is_bound_to_its_group() {
        let mut w = w();
        let blob = seal(&[5; 32], GID, 5, &w.a, "k", "x", now(), 0).unwrap();
        assert_eq!(open(&w.keys, &mut w.seen, "00112233445566778899aabbccddeeff", &dev(&w.a), &blob, now()), Err(LiveErr::Auth));
        assert!(open(&w.keys, &mut w.seen, GID, &dev(&w.a), &blob, now()).is_ok());
    }

    #[test]
    fn one_member_cannot_speak_as_another() {
        let mut w = w();
        // Bob seals honestly but the relay says it came from Alice: the additional data names the wrong device.
        let blob = seal(&[5; 32], GID, 5, &w.b, "k", "x", now(), 0).unwrap();
        assert_eq!(open(&w.keys, &mut w.seen, GID, &dev(&w.a), &blob, now()), Err(LiveErr::Auth));
        // Bob goes further: names Alice's device in the additional data and the signed part, but can only sign with his own key.
        let a_pub = w.a.verifying_key().to_bytes();
        let forged = seal_as(&[5; 32], GID, 5, &a_pub, &w.b, "k", "evil", now(), 0).unwrap();
        assert_eq!(open(&w.keys, &mut w.seen, GID, &dev(&w.a), &forged, now()), Err(LiveErr::Signature));
        // Honest Bob is heard as Bob.
        let ok = seal(&[5; 32], GID, 5, &w.b, "k", "x", now(), 0).unwrap();
        assert_eq!(open(&w.keys, &mut w.seen, GID, &dev(&w.b), &ok, now()).unwrap().account, "bob");
    }

    #[test]
    fn re_sealing_somebody_elses_signed_event_under_a_new_nonce_fails() {
        let mut w = w();
        let a = dev(&w.a);
        let blob = seal(&[5; 32], GID, 5, &w.a, "k", "x", now(), 0).unwrap();
        // A member holds the key, so can open Alice's event and seal the same plaintext again with a fresh nonce.
        let nonce: [u8; 12] = blob[9..21].try_into().unwrap();
        let device = w.a.verifying_key().to_bytes();
        let pt = ChaCha20Poly1305::new((&[5u8; 32]).into()).decrypt(Nonce::from_slice(&nonce), Payload { msg: &blob[21..], aad: &aad(GID, 5, &device) }).unwrap();
        let n2 = [7u8; 12];
        let ct = ChaCha20Poly1305::new((&[5u8; 32]).into()).encrypt(Nonce::from_slice(&n2), Payload { msg: &pt, aad: &aad(GID, 5, &device) }).unwrap();
        let mut again = vec![VER]; again.extend(5u64.to_be_bytes()); again.extend(n2); again.extend(ct);
        assert_eq!(open(&w.keys, &mut w.seen, GID, &a, &again, now()), Err(LiveErr::Signature), "the nonce is under the signature");
    }

    #[test]
    fn a_removed_member_is_not_heard_even_with_a_key() {
        let mut w = w();
        let blob = seal(&[5; 32], GID, 5, &w.b, "k", "x", now(), 0).unwrap();
        // Bob is removed: the next epoch's roster has only Alice. He cannot make the new key, but an event of his from the
        // previous epoch, whose key we still hold for a moment, must not be accepted either.
        let mut roster = w.keys.roster.clone();
        roster.remove(&dev(&w.b));
        w.keys.advance(6, [6; 32], roster);
        assert_eq!(open(&w.keys, &mut w.seen, GID, &dev(&w.b), &blob, now()), Err(LiveErr::Roster));
        let late = seal(&[6; 32], GID, 6, &w.b, "k", "x", now(), 0).unwrap();
        assert_eq!(open(&w.keys, &mut w.seen, GID, &dev(&w.b), &late, now()), Err(LiveErr::Roster));
        // Alice still is.
        let ok = seal(&[5; 32], GID, 5, &w.a, "k", "x", now(), 0).unwrap();
        assert!(open(&w.keys, &mut w.seen, GID, &dev(&w.a), &ok, now()).is_ok(), "previous epoch, still in the roster");
    }

    #[test]
    fn a_replayed_event_is_taken_once() {
        let mut w = w();
        let blob = seal(&[5; 32], GID, 5, &w.a, "k", "x", now(), 0).unwrap();
        assert!(open(&w.keys, &mut w.seen, GID, &dev(&w.a), &blob, now()).is_ok());
        assert_eq!(open(&w.keys, &mut w.seen, GID, &dev(&w.a), &blob, now()), Err(LiveErr::Replay));
        assert_eq!(w.seen.len(), 1);
        // A failed attempt does not burn the nonce of a good event.
        let blob2 = seal(&[5; 32], GID, 5, &w.a, "k", "y", now(), 0).unwrap();
        let mut bad = blob2.clone(); let l = bad.len(); bad[l - 1] ^= 1;
        assert_eq!(open(&w.keys, &mut w.seen, GID, &dev(&w.a), &bad, now()), Err(LiveErr::Auth));
        assert!(open(&w.keys, &mut w.seen, GID, &dev(&w.a), &blob2, now()).is_ok());
    }

    #[test]
    fn any_change_to_a_sealed_event_is_refused() {
        let mut w = w();
        let a = dev(&w.a);
        let blob = seal(&[5; 32], GID, 5, &w.a, "k", "hello", now(), 0).unwrap();
        for i in 0..blob.len() {
            let mut t = blob.clone();
            t[i] ^= 0x40;
            let r = open(&w.keys, &mut w.seen, GID, &a, &t, now());
            assert!(r.is_err(), "flipping byte {i} was accepted");
        }
        // truncated, extended, empty, wrong version
        assert!(open(&w.keys, &mut w.seen, GID, &a, &blob[..blob.len() - 1], now()).is_err());
        let mut longer = blob.clone(); longer.push(0);
        assert!(open(&w.keys, &mut w.seen, GID, &a, &longer, now()).is_err());
        assert_eq!(open(&w.keys, &mut w.seen, GID, &a, &[], now()), Err(LiveErr::Malformed));
        let mut v = blob.clone(); v[0] = 2;
        assert_eq!(open(&w.keys, &mut w.seen, GID, &a, &v, now()), Err(LiveErr::Malformed));
        // a sender id that is not a device id at all
        assert!(open(&w.keys, &mut w.seen, GID, "nonsense", &blob, now()).is_err());
        assert!(open(&w.keys, &mut w.seen, GID, &a, &blob, now()).is_ok(), "the original is still fine");
    }

    #[test]
    fn size_kind_and_time_limits_hold() {
        let mut w = w();
        let a = dev(&w.a);
        assert_eq!(seal(&[5; 32], GID, 5, &w.a, "k", &"x".repeat(MAX_LIVE + 1), now(), 0), Err(LiveErr::Oversized));
        assert_eq!(seal(&[5; 32], GID, 5, &w.a, "Bad Kind", "x", now(), 0), Err(LiveErr::Oversized));
        let max = seal(&[5; 32], GID, 5, &w.a, "k", &"x".repeat(MAX_LIVE), now(), 0).unwrap();
        assert!(max.len() <= commons_proto::LIVE_MAX_BLOB, "the biggest event ({}) fits what the relay takes", max.len());
        assert!(open(&w.keys, &mut w.seen, GID, &a, &max, now()).is_ok());
        // a blob bigger than anything we send
        assert_eq!(open(&w.keys, &mut w.seen, GID, &a, &vec![1; commons_proto::LIVE_MAX_BLOB + 1], now()), Err(LiveErr::Malformed));
        // old and far-future events (by the signed time)
        let old = seal(&[5; 32], GID, 5, &w.a, "k", "x", now() - 3600, 0).unwrap();
        assert_eq!(open(&w.keys, &mut w.seen, GID, &a, &old, now()), Err(LiveErr::Stale));
        let ahead = seal(&[5; 32], GID, 5, &w.a, "k", "x", now() + 3600, 0).unwrap();
        assert_eq!(open(&w.keys, &mut w.seen, GID, &a, &ahead, now()), Err(LiveErr::Stale));
    }

    #[test]
    fn sealing_twice_never_repeats_a_nonce_and_looks_different() {
        let w = w();
        let a = seal(&[5; 32], GID, 5, &w.a, "k", "same", 100, 0).unwrap();
        let b = seal(&[5; 32], GID, 5, &w.a, "k", "same", 100, 0).unwrap();
        assert_ne!(a[9..21], b[9..21]);
        assert_ne!(a[21..], b[21..]);
    }
}
