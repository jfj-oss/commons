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
use std::collections::{BTreeMap, HashMap, VecDeque};

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
/// Events from an epoch we have not reached: how many are held, and for how many seconds.
const FUTURE_MAX: usize = 64;
const FUTURE_SECS: u64 = 10;

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
pub struct Opened { pub account: String, pub device: String, pub kind: String, pub data: String, pub ts: u64 }

fn aad(gid: &str, epoch: u64, device: &[u8; 32]) -> Vec<u8> {
    let mut a = DOM.to_vec();
    a.extend((gid.len() as u16).to_be_bytes()); a.extend(gid.as_bytes());
    a.extend(epoch.to_be_bytes()); a.extend(device);
    a
}
fn signed_part(gid: &str, epoch: u64, device: &[u8; 32], nonce: &[u8; 12], kind: &str, ts: u64, data: &str) -> Vec<u8> {
    let mut m = aad(gid, epoch, device);
    m.extend(nonce);
    m.push(kind.len() as u8); m.extend(kind.as_bytes());
    m.extend(ts.to_be_bytes());
    m.extend((data.len() as u16).to_be_bytes()); m.extend(data.as_bytes());
    m
}

/// Seal one event. `signer` is this device's key; its public half is the device id.
pub fn seal(key: &[u8; 32], gid: &str, epoch: u64, signer: &SigningKey, kind: &str, data: &str, ts: u64) -> Result<Vec<u8>, LiveErr> {
    if !valid_live_kind(kind) || data.len() > MAX_LIVE { return Err(LiveErr::Oversized); }
    let device = signer.verifying_key().to_bytes();
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let sig = signer.sign(&signed_part(gid, epoch, &device, &nonce, kind, ts, data));
    let mut pt = sig.to_bytes().to_vec();
    pt.push(kind.len() as u8); pt.extend(kind.as_bytes());
    pt.extend(ts.to_be_bytes());
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
    // sig(64) | kind len(1) | kind | ts(8) | data len(2) | data | zero padding
    if pt.len() < 64 + 1 { return Err(LiveErr::Malformed); }
    let sig = Signature::from_slice(&pt[..64]).map_err(|_| LiveErr::Malformed)?;
    let kl = pt[64] as usize;
    let mut p = 65;
    let take = |p: &mut usize, n: usize| -> Result<&[u8], LiveErr> { if *p + n > pt.len() { Err(LiveErr::Malformed) } else { let s = &pt[*p..*p + n]; *p += n; Ok(s) } };
    let kind = std::str::from_utf8(take(&mut p, kl)?).map_err(|_| LiveErr::Malformed)?.to_string();
    let ts = u64::from_be_bytes(take(&mut p, 8)?.try_into().unwrap());
    let dl = u16::from_be_bytes(take(&mut p, 2)?.try_into().unwrap()) as usize;
    if dl > MAX_LIVE { return Err(LiveErr::Oversized); }
    let data = std::str::from_utf8(take(&mut p, dl)?).map_err(|_| LiveErr::Malformed)?.to_string();
    if !valid_live_kind(&kind) || pt[p..].iter().any(|b| *b != 0) { return Err(LiveErr::Malformed); }
    let vk = VerifyingKey::from_bytes(&device).map_err(|_| LiveErr::Malformed)?;
    vk.verify(&signed_part(gid, epoch, &device, &nonce, &kind, ts, &data), &sig).map_err(|_| LiveErr::Signature)?;
    if ts + LIVE_MAX_AGE < now || ts > now + LIVE_MAX_AHEAD { return Err(LiveErr::Stale); }
    if !seen.first(nonce, now) { return Err(LiveErr::Replay); }
    Ok(Opened { account: account.clone(), device: from.to_string(), kind, data, ts })
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
    future: Vec<(String, String, Vec<u8>, u64)>,
    /// Set when the app runs its own sender and poller threads: the ordinary round then only sends.
    pub background: bool,
    /// What the channel did, for tests and the debug view.
    pub stats: LiveStats,
}
#[derive(Default, Clone, Debug)]
pub struct LiveStats { pub sealed: u64, pub taken: u64, pub dropped: u64, pub posts: u64, pub polls: u64 }

fn now_ms() -> u128 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0) }

/// One round of work for the channel, planned with the client held and run without it.
pub struct LivePlan {
    api: Api,
    key: SigningKey,
    sends: Vec<(String, Vec<Vec<u8>>)>,
    poll: Option<LivePollReq>,
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
    pub(crate) fn live_chan_on(&self) -> bool { !matches!(self.live.support, Support::No(_)) }
    pub fn live_support(&self) -> Support { self.live.support }
    pub fn live_stats(&self) -> LiveStats { self.live.stats.clone() }
    pub fn set_live_background(&mut self, on: bool) { self.live.background = on; }

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
        match seal(&c.key, gid, c.epoch, &dk, kind, data, now_secs()) {
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
        if !self.has_identity() || self.st.self_revoked { return None; }
        let now_m = now_ms();
        let mut sends: BTreeMap<String, Vec<Vec<u8>>> = BTreeMap::new();
        while send {
            let Some((gid, b, at)) = self.live.wire.pop_front() else { break };
            if now_m < at + 3000 && self.live_chan_on() { sends.entry(gid).or_default().push(b); }
        }
        let poll = poll.then(|| self.live_poll_req()).flatten();
        if sends.is_empty() && poll.is_none() { return None; }
        let key = self.dev_key().ok()?;
        Some(LivePlan { api: self.api.clone(), key, sends: sends.into_iter().collect(), poll })
    }

    /// The poll to make now: none if the relay is known not to carry the channel and it is not yet time to ask again.
    pub fn live_poll_req(&self) -> Option<LivePollReq> {
        if let Support::No(until) = self.live.support { if now_secs() < until { return None; } }
        // Only rooms we can read. A room we have not polled before starts at "now" (u64::MAX), not at what is buffered.
        let cursors: BTreeMap<String, u64> = self.st.groups.values()
            .filter(|g| g.kind == Kind::Channel && !g.removed && !g.broken)
            .map(|g| (g.gid.clone(), self.live.cursors.get(&g.gid).copied().unwrap_or(u64::MAX))).collect();
        if cursors.is_empty() { return None; }
        Some(LivePollReq { boot: self.live.boot.clone(), cursors, wait_ms: 0 })
    }

    /// Apply what the network part found. Local work only; nothing here is saved.
    pub fn live_finish(&mut self, d: LiveDone) {
        self.live.stats.posts += d.posts;
        self.live.stats.polls += d.polls;
        match d.support {
            Some(true) => self.live.support = Support::Yes,
            Some(false) => { self.live.support = Support::No(now_secs() + LIVE_RECHECK_SECS); self.live.wire.clear(); self.live.future.clear(); }
            None => {}
        }
        let now = now_secs();
        // Events that came early (sealed under an epoch we had not reached) get another look.
        let held = std::mem::take(&mut self.live.future);
        let mut incoming: Vec<(String, String, Vec<u8>, u64)> = held.into_iter().filter(|h| h.3 + FUTURE_SECS >= now).collect();
        if let Some(r) = d.poll {
            if r.boot != self.live.boot { self.live.boot = r.boot.clone(); self.live.cursors.clear(); }
            for (gid, g) in r.groups {
                if !self.st.groups.get(&gid).is_some_and(|s| s.kind == Kind::Channel && !s.removed) { continue; }
                self.live.cursors.insert(gid.clone(), g.next);
                for it in g.events {
                    if let Ok(b) = unb64(&it.data) { incoming.push((gid.clone(), it.from, b, now)); }
                }
            }
        }
        if incoming.is_empty() { return; }
        let me = self.my_device();
        let mut fresh: BTreeMap<String, bool> = BTreeMap::new();
        for (gid, from, blob, at) in incoming {
            if from == me { continue; }
            let ok = *fresh.entry(gid.clone()).or_insert_with(|| self.live_refresh(&gid));
            if !ok { self.live.stats.dropped += 1; continue; }
            let keys = self.live.keys.get(&gid).cloned().unwrap_or_default();
            match open(&keys, &mut self.live.seen, &gid, &from, &blob, now) {
                Ok(o) => {
                    self.live.stats.taken += 1;
                    self.take_live_limited(&gid, &o.account, o.kind, o.data, o.ts, LIVE_CHAN_TAKE_PER_SEC);
                }
                Err(LiveErr::Future) => {
                    if self.live.future.len() < FUTURE_MAX { self.live.future.push((gid, from, blob, at)); } else { self.live.stats.dropped += 1; }
                }
                Err(_) => self.live.stats.dropped += 1,
            }
        }
    }

    /// Plan, run and apply the channel once, holding nothing in between that matters. For tests and for the
    /// synchronous `tick`: the app's threads do the same in two halves.
    pub fn live_tick(&mut self) {
        self.flush_live();
        if let Some(p) = self.live_plan(true) { let d = p.run(0); self.live_finish(d); }
    }
}
