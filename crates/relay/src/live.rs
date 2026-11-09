//! The live channel's buffer: sealed events that are held in memory for a few seconds and then forgotten.
//!
//! A room's counter is kept (a few bytes) even when its ring is empty, so a sequence number never repeats within one run.
//! Nothing here touches the database or the disk. A room's events sit in a ring that is capped by age, by count and by
//! bytes, in total and per sending device, so that no one can use the relay's memory or crowd the others out. The
//! relay cannot read an event: it is sealed with a key only the room's members can derive (docs/THREAT-MODEL.md).
use std::collections::{HashMap, VecDeque};

/// Limits of the live buffer. All of them are per relay process and are lost on restart, by design.
#[derive(Clone, Debug)]
pub struct LiveLimits {
    /// How long an event is kept for slow pollers.
    pub ttl_ms: u64,
    pub group_events: usize,
    pub group_bytes: usize,
    /// What one device may have in one room's ring at once.
    pub sender_events: usize,
    pub sender_bytes: usize,
    /// Events one device may post to one room per second, and the whole relay may hold at once.
    pub per_sec: u32,
    pub total_bytes: usize,
    /// Most events and bytes one poll answer carries per room.
    pub answer_events: usize,
    pub answer_bytes: usize,
}
impl Default for LiveLimits {
    fn default() -> Self {
        LiveLimits { ttl_ms: 30_000, group_events: 1500, group_bytes: 3 << 20, sender_events: 450, sender_bytes: 1 << 20, per_sec: 30, total_bytes: 64 << 20, answer_events: 200, answer_bytes: 512 << 10 }
    }
}

struct Ent { seq: u64, from: String, data: Vec<u8>, at: u64 }
#[derive(Default)]
struct Ring { next: u64, q: VecDeque<Ent>, bytes: usize, per: HashMap<String, (usize, usize)> }

#[derive(Debug, PartialEq, Eq)]
pub enum Push { Taken, RateLimited, Full, TooBig }

pub struct LiveHub {
    pub boot: String,
    pub lim: LiveLimits,
    rings: HashMap<String, Ring>,
    total: usize,
    rate: HashMap<String, (u64, u32)>,
}

impl LiveHub {
    pub fn new(boot: String, lim: LiveLimits) -> Self { LiveHub { boot, lim, rings: HashMap::new(), total: 0, rate: HashMap::new() } }

    fn expire(&mut self, gid: &str, now: u64) {
        let ttl = self.lim.ttl_ms;
        let Some(r) = self.rings.get_mut(gid) else { return };
        while r.q.front().is_some_and(|e| e.at + ttl <= now) {
            let e = r.q.pop_front().unwrap();
            r.bytes -= e.data.len();
            self.total -= e.data.len();
            if let Some(p) = r.per.get_mut(&e.from) { p.0 -= 1; p.1 -= e.data.len(); if p.0 == 0 { r.per.remove(&e.from); } }
        }
    }
    /// Forget everything that has expired, in every room. Run now and then so that a quiet room costs nothing.
    pub fn sweep(&mut self, now: u64) {
        let gids: Vec<String> = self.rings.keys().cloned().collect();
        for g in gids { self.expire(&g, now); }
        let sec = now / 1000;
        if self.rate.len() > 4096 { self.rate.retain(|_, v| v.0 + 2 >= sec); }
    }
    /// Offer one sealed event from `device` to room `gid`. `now` is milliseconds from any steady clock.
    pub fn push(&mut self, gid: &str, device: &str, data: Vec<u8>, now: u64) -> Push {
        if data.is_empty() || data.len() > commons_proto::LIVE_MAX_BLOB { return Push::TooBig; }
        let sec = now / 1000;
        let rk = format!("{device}|{gid}");
        let e = self.rate.entry(rk).or_insert((sec, 0));
        if e.0 != sec { *e = (sec, 0); }
        if e.1 >= self.lim.per_sec { return Push::RateLimited; }
        e.1 += 1;
        self.expire(gid, now);
        if self.total + data.len() > self.lim.total_bytes { return Push::Full; }
        let r = self.rings.entry(gid.to_string()).or_default();
        let (se, sb) = r.per.get(device).copied().unwrap_or((0, 0));
        if r.q.len() >= self.lim.group_events || r.bytes + data.len() > self.lim.group_bytes || se >= self.lim.sender_events || sb + data.len() > self.lim.sender_bytes {
            return Push::Full;
        }
        r.next += 1;
        let n = data.len();
        r.q.push_back(Ent { seq: r.next, from: device.to_string(), data, at: now });
        r.bytes += n; self.total += n;
        let p = r.per.entry(device.to_string()).or_default();
        p.0 += 1; p.1 += n;
        Push::Taken
    }
    /// What is newer than `after` in a room, other than `skip_device`'s own events, within the answer limits.
    /// Returns the events and the cursor to use next.
    pub fn since(&mut self, gid: &str, after: u64, skip_device: &str, now: u64) -> (Vec<(u64, String, Vec<u8>)>, u64) {
        self.expire(gid, now);
        let Some(r) = self.rings.get(gid) else { return (vec![], after) };
        let mut out = vec![];
        let mut bytes = 0usize;
        let mut next = r.next;
        let mut last = after;
        for e in r.q.iter().filter(|e| e.seq > after) {
            if e.from == skip_device { last = e.seq; continue; }
            if out.len() >= self.lim.answer_events || (!out.is_empty() && bytes + e.data.len() > self.lim.answer_bytes) { next = last; break; }
            bytes += e.data.len();
            last = e.seq;
            out.push((e.seq, e.from.clone(), e.data.clone()));
        }
        (out, next)
    }
    /// The newest sequence number of a room in this run (0 if it has had none).
    pub fn head_of(&self, gid: &str) -> u64 { self.rings.get(gid).map_or(0, |r| r.next) }
    pub fn total_bytes(&self) -> usize { self.total }
    pub fn rooms(&self) -> usize { self.rings.values().filter(|r| !r.q.is_empty()).count() }
    pub fn events_in(&self, gid: &str) -> usize { self.rings.get(gid).map_or(0, |r| r.q.len()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hub(l: LiveLimits) -> LiveHub { LiveHub::new("b".into(), l) }
    #[test]
    fn events_expire_and_memory_is_released() {
        let mut h = hub(LiveLimits::default());
        for i in 0..10 { assert_eq!(h.push("g", "d1", vec![1; 100], i * 10), Push::Taken); }
        assert_eq!((h.events_in("g"), h.total_bytes()), (10, 1000));
        let (ev, next) = h.since("g", 0, "other", 500);
        assert_eq!((ev.len(), next), (10, 10));
        let (ev, _) = h.since("g", 0, "other", 40_000);
        assert!(ev.is_empty(), "held for 30 s, not longer");
        assert_eq!((h.total_bytes(), h.rooms()), (0, 0));
    }
    #[test]
    fn caps_hold_per_sender_per_room_and_overall() {
        let l = LiveLimits { sender_events: 5, group_events: 8, per_sec: 1000, total_bytes: 6_000, ..Default::default() };
        let mut h = hub(l);
        for _ in 0..5 { assert_eq!(h.push("g", "a", vec![0; 10], 1), Push::Taken); }
        assert_eq!(h.push("g", "a", vec![0; 10], 1), Push::Full, "one device's share");
        for _ in 0..3 { assert_eq!(h.push("g", "b", vec![0; 10], 1), Push::Taken); }
        assert_eq!(h.push("g", "c", vec![0; 10], 1), Push::Full, "the room's count");
        assert_eq!(h.push("g2", "c", vec![0; 4_000], 1), Push::Taken);
        assert_eq!(h.push("g3", "c", vec![0; 2_500], 1), Push::Full, "the relay's bytes");
        assert_eq!(h.push("g", "a", vec![], 1), Push::TooBig);
        assert_eq!(h.push("g", "a", vec![0; commons_proto::LIVE_MAX_BLOB + 1], 1), Push::TooBig);
        // sender bytes
        let mut h = hub(LiveLimits { sender_bytes: 300, per_sec: 1000, ..Default::default() });
        assert_eq!(h.push("g", "a", vec![0; 200], 1), Push::Taken);
        assert_eq!(h.push("g", "a", vec![0; 200], 1), Push::Full);
    }
    #[test]
    fn posting_is_rate_limited_per_device_and_room() {
        let mut h = hub(LiveLimits { per_sec: 3, ..Default::default() });
        let n = (0..10).filter(|_| h.push("g", "a", vec![0; 5], 5_000) == Push::Taken).count();
        assert_eq!(n, 3);
        assert_eq!(h.push("g", "b", vec![0; 5], 5_000), Push::Taken, "another device has its own allowance");
        assert_eq!(h.push("h", "a", vec![0; 5], 5_000), Push::Taken, "and so does another room");
        assert_eq!(h.push("g", "a", vec![0; 5], 6_000), Push::Taken, "the next second");
    }
    #[test]
    fn a_poll_skips_the_askers_own_events_and_pages() {
        let mut h = hub(LiveLimits { answer_events: 2, per_sec: 1000, ..Default::default() });
        for i in 0..6 { h.push("g", if i % 2 == 0 { "me" } else { "you" }, vec![i as u8; 4], 1); }
        let (ev, next) = h.since("g", 0, "me", 2);
        assert_eq!((ev.iter().map(|e| e.0).collect::<Vec<_>>(), next), (vec![2, 4], 5), "only the other device's, two to an answer");
        let (ev, next) = h.since("g", next, "me", 2);
        assert_eq!((ev.iter().map(|e| e.0).collect::<Vec<_>>(), next), (vec![6], 6));
        let (ev, next) = h.since("g", 6, "me", 2);
        assert_eq!((ev.len(), next), (0, 6));
    }
}
