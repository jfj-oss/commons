//! Room history for a new device of this account (docs/history-transfer.md). A device that has just been paired asks the account's
//! other devices; the person says yes on one of them (or said so when approving the pairing), and that device sends its rooms'
//! messages in chunks sealed to the new device's key, through the relay's per-device inbox. The relay carries ciphertext and
//! cannot tell the chunks from Welcomes except by size; no group and no other member is involved; both sides resume after a restart.
use crate::client::{rand_hex, Client};
use crate::state::*;
use crate::transport::relay_status;
use anyhow::{anyhow, bail, Result};
use commons_proto::{b64, now_secs, sha256, unb64, DeviceCert};
use ed25519_dalek::{Signer as _, Verifier as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// Most plaintext in one chunk (the inbox takes items up to 1 MiB; sealed and base64 this stays well under).
pub const CHUNK_PLAIN: usize = 512 * 1024;
/// Most plaintext in one transfer; beyond it the oldest messages stay on the old device.
pub const TOTAL_CAP: usize = 64 << 20;
/// Chunks are padded to a multiple of this before sealing, so the relay sees sizes in these steps.
const PAD: usize = 4096;
/// How long a yes given with a pairing approval covers a request from the device that pairing produced.
pub const PREAPPROVE_SECS: u64 = 900;
/// A request older than this (or further ahead) is not believed.
const REQ_MAX_AGE: u64 = 600;
/// Chunks handed to the inbox at a time.
const IN_FLIGHT: usize = 4;
/// How often a device that has nothing yet asks again, and for how long it keeps asking.
const ASK_EVERY: u64 = 300;
const ASK_FOR: u64 = 86400;
/// The first bytes of a chunk item in the inbox (MLS messages start with a version number, JSON with a brace).
pub const MAGIC: &[u8] = b"HIST1";
const GID_PREFIX: &str = "history:";

/// The request. `community` and `reason` are there for apps before 0.3.6: they read every JSON inbox item as a reply to a join request,
/// and a well-formed reply about a community they are not joining is dropped in silence, so the request costs them nothing, not even a notice.
#[derive(Serialize, Deserialize)]
struct ReqWire { t: String, v: u8, community: String, reason: String, cert: DeviceCert, ts: u64, sig: String }
fn req_community(device: &str) -> String { hex::encode(sha256(&[b"commons/history-req/community", device.as_bytes()])) }
#[derive(Serialize, Deserialize)]
struct ChunkWire { tid: String, i: u32, n: u32, cert: DeviceCert, blob: String, sig: String }
#[derive(Serialize, Deserialize)]
struct ChunkPlain { v: u8, tid: String, i: u32, n: u32, omitted: u64, slices: Vec<Slice> }
#[derive(Serialize, Deserialize, Clone)]
struct Slice { gid: String, community: String, unread: usize, msgs: Vec<Msg> }

fn req_msg(account: &str, device: &str, ts: u64) -> Vec<u8> { sha256(&[b"commons/history-req/v1", account.as_bytes(), device.as_bytes(), &ts.to_be_bytes()]).to_vec() }
fn chunk_msg(tid: &str, i: u32, n: u32, blob: &[u8]) -> Vec<u8> { sha256(&[b"commons/history-chunk/v1", tid.as_bytes(), &i.to_be_bytes(), &n.to_be_bytes(), &sha256(&[blob])]).to_vec() }
fn chunk_aad(tid: &str, i: u32, n: u32, device: &str) -> Vec<u8> { sha256(&[b"commons/history/v1", tid.as_bytes(), &i.to_be_bytes(), &n.to_be_bytes(), device.as_bytes()]).to_vec() }

fn pad(mut v: Vec<u8>) -> Vec<u8> {
    let mut out = (v.len() as u32).to_be_bytes().to_vec();
    out.append(&mut v);
    out.resize(out.len().div_ceil(PAD) * PAD, 0);
    out
}
fn unpad(v: &[u8]) -> Option<&[u8]> {
    if v.len() < 4 { return None; }
    let n = u32::from_be_bytes(v[..4].try_into().ok()?) as usize;
    v.get(4..4 + n)
}

fn verify_sig(cert: &DeviceCert, msg: &[u8], sig_hex: &str) -> bool {
    (|| -> Option<bool> {
        let vk = ed25519_dalek::VerifyingKey::from_bytes(&commons_proto::unhex32(&cert.device).ok()?).ok()?;
        let sig = ed25519_dalek::Signature::from_slice(&hex::decode(sig_hex).ok()?).ok()?;
        Some(vk.verify(msg, &sig).is_ok())
    })().unwrap_or(false)
}

impl Client {
    /// Is this a device of my account that I may believe: its cert is signed by my account key, it is not me, and it is not revoked.
    fn own_other_device(&self, cert: &DeviceCert) -> Result<()> {
        if cert.account != self.my_account() { bail!("another account's device"); }
        cert.verify().map_err(|e| anyhow!("bad device certificate: {e}"))?;
        if cert.device == self.my_device() { bail!("this device"); }
        if self.st.revoked.contains_key(&cert.device) { bail!("a revoked device"); }
        Ok(())
    }

    // ------------------------------------------------------------------ the new device: asking, receiving
    /// The request item this device posts to the account's other devices.
    pub fn history_request_bytes(&self) -> Result<Vec<u8>> {
        let cert = self.cert()?;
        let ts = now_secs();
        let sig = hex::encode(self.dev_key()?.sign(&req_msg(&cert.account, &cert.device, ts)).to_bytes());
        Ok(serde_json::to_vec(&ReqWire { t: "hist-req".into(), v: 1, community: req_community(&cert.device), reason: "hist-req".into(), cert, ts, sig })?)
    }

    /// Ask every other unrevoked device of the account (from the directory the rounds keep fresh); now and then again while nothing arrives.
    pub(crate) fn history_ask(&mut self, now: u64) {
        if !self.st.history_wanted || self.st.history_in.is_some() || now < self.st.history_asked + ASK_EVERY { return; }
        if now > self.st.history_wanted_since + ASK_FOR { self.st.history_wanted = false; return; }
        let me = self.my_account();
        let Some((_, list)) = self.dir_cache.get(&me).cloned() else { self.dir_wanted.insert(me); return };
        let Ok(req) = self.history_request_bytes() else { return };
        let my_dev = self.my_device();
        let others: Vec<String> = list.iter().filter(|(c, revoked)| !revoked && c.device != my_dev && !self.st.revoked.contains_key(&c.device)).map(|(c, _)| c.device.clone()).collect();
        if others.is_empty() { return; }
        // one request per device, replacing one still unposted from last time
        self.st.welcome_out.retain(|w| w.gid != "history-req");
        for d in others { self.st.welcome_out.push(WelcomeOut { id: rand_hex(16), device: d, data: b64(&req), gid: "history-req".into() }); }
        self.st.history_asked = now;
    }

    /// A chunk from the inbox (after `MAGIC`). Checked, opened, merged, and counted; anything that fails is dropped with one notice.
    pub(crate) fn history_take_chunk(&mut self, data: &[u8]) {
        let Ok(w) = serde_json::from_slice::<ChunkWire>(data) else { self.notice("history", "ignored a history chunk that could not be read"); return };
        if let Err(e) = self.own_other_device(&w.cert) { self.notice("history", format!("ignored a history chunk from {e}")); return; }
        let Ok(blob) = unb64(&w.blob) else { self.notice("history", "ignored a history chunk that could not be read"); return };
        if w.n == 0 || w.i >= w.n || w.n > 4096 || !verify_sig(&w.cert, &chunk_msg(&w.tid, w.i, w.n, &blob), &w.sig) { self.notice("history", "ignored a history chunk with a bad signature"); return; }
        let me_dev = self.my_device();
        let Some(padded) = self.dev_key().ok().and_then(|k| crate::seal::open_with(&k, &chunk_aad(&w.tid, w.i, w.n, &me_dev), &blob)) else { self.notice("history", "ignored a history chunk that was not sealed to this device"); return };
        let Some(plain) = unpad(&padded) else { self.notice("history", "ignored a damaged history chunk"); return };
        let Ok(p) = serde_json::from_slice::<ChunkPlain>(plain) else { self.notice("history", "ignored a damaged history chunk"); return };
        if p.v != 1 || p.tid != w.tid || p.i != w.i || p.n != w.n { self.notice("history", "ignored a history chunk whose contents did not match its header"); return; }
        // one transfer at a time: a second device's answer waits until this one is done (or is a repeat of a finished one)
        match &self.st.history_in {
            Some(h) if h.tid != p.tid && h.done == 0 => { self.notice("history", format!("a second device ({}...) also sent history; only the first is taken", &w.cert.device[..8])); return; }
            Some(h) if h.tid != p.tid => { self.st.history_in = None; }
            _ => {}
        }
        let h = self.st.history_in.get_or_insert_with(|| HistoryIn { tid: p.tid.clone(), from: w.cert.device.clone(), n: p.n, got: BTreeSet::new(), msgs: 0, omitted: 0, started: now_secs(), done: 0, dismissed: false });
        if h.done != 0 || h.got.contains(&p.i) { return; }
        let mut added = 0u64;
        let me = self.my_account();
        for s in p.slices {
            if !crate::envelope::is_gid(&s.gid) { continue; }
            let log = self.st.chans.entry(s.gid.clone()).or_default();
            let unread_before = log.msgs.len().saturating_sub(log.read_upto);
            let have: HashSet<String> = log.msgs.iter().map(|m| m.id.clone()).collect();
            let mut new_ids = vec![];
            for mut m in s.msgs {
                if m.id.is_empty() || have.contains(&m.id) || m.status == "sending" { continue; }
                if m.status != "system" { m.status = if m.sender == me { "sent".into() } else { "received".into() }; }
                new_ids.push(m.id.clone());
                log.msgs.push(m);
                added += 1;
            }
            // storage order is reading order (relay order, local unsent ones at the end), so the unread count stays a tail
            let (mut sent, pending): (Vec<Msg>, Vec<Msg>) = std::mem::take(&mut log.msgs).into_iter().partition(|m| m.seq > 0 || m.status == "system");
            sent.sort_by_key(|m| (if m.seq > 0 { m.seq } else { u64::MAX }, m.ts));
            sent.extend(pending);
            log.msgs = sent;
            let unread = unread_before.max(s.unread.min(log.msgs.len()));
            log.read_upto = log.msgs.len() - unread;
            for id in new_ids { Client::drain_orphans(log, &id, &s.gid); }
        }
        let h = self.st.history_in.as_mut().unwrap();
        h.got.insert(p.i);
        h.msgs += added;
        h.omitted = p.omitted;
        let complete = h.got.len() as u32 >= h.n;
        if complete { h.done = now_secs(); }
        let (msgs, omitted) = (h.msgs, h.omitted);
        self.st.history_wanted = false;
        if complete {
            self.notice("history", if omitted > 0 { format!("history from your other device arrived: {msgs} messages; the oldest {omitted} stayed on the other device") } else { format!("history from your other device arrived: {msgs} messages") });
        }
        self.activity += 1;
    }

    /// Stop showing the note about a finished (or unanswered) transfer.
    pub fn history_dismiss(&mut self) -> Result<()> {
        if let Some(h) = self.st.history_in.as_mut() { h.dismissed = true; }
        self.st.history_wanted = false;
        self.save()
    }

    // ------------------------------------------------------------------ the old device: requests, sending
    /// A request from the inbox. Believed only from an unrevoked device of this account, signed by that device, and fresh.
    pub(crate) fn history_take_request(&mut self, data: &[u8]) {
        let Ok(r) = serde_json::from_slice::<ReqWire>(data) else { self.notice("history", "ignored a malformed history request"); return };
        if r.v != 1 { return; }
        if let Err(e) = self.own_other_device(&r.cert) { self.notice("history", format!("ignored a history request from {e}")); return; }
        let now = now_secs();
        if r.ts + REQ_MAX_AGE < now || r.ts > now + REQ_MAX_AGE { self.notice("history", "ignored a history request that was too old"); return; }
        if !verify_sig(&r.cert, &req_msg(&r.cert.account, &r.cert.device, r.ts), &r.sig) { self.notice("history", "ignored a history request with a bad signature"); return; }
        let dev = r.cert.device.clone();
        // already on its way, or already sent: the device asks again after a restart while it has nothing, which the queue answers by itself
        if self.st.history_out.iter().any(|h| h.device == dev) { return; }
        let preapproved = now < self.st.history_preapproved && r.cert.created + PREAPPROVE_SECS + REQ_MAX_AGE >= self.st.history_preapproved;
        self.st.history_requests.insert(dev.clone(), HistoryRequest { ts: now, cert: r.cert });
        if preapproved {
            match self.history_send(&dev) {
                Ok(_) => { self.notice("device", format!("sending this device's message history to your new device {}... (you said yes when approving it)", &dev[..8])); }
                Err(e) => self.notice("history", format!("could not send history to {}...: {e}", &dev[..8])),
            }
        } else {
            self.notice("device", format!("your new device {}... asks for this device's message history. Open Devices to send it, or leave it.", &dev[..8]));
        }
        self.activity += 1;
    }

    /// The yes given with a pairing approval: a request from the device that pairing produces is sent without asking again.
    pub fn history_preapprove(&mut self) { self.st.history_preapproved = now_secs() + PREAPPROVE_SECS; }

    /// What a transfer from this device would carry: rooms and messages.
    pub fn history_summary(&self) -> (usize, usize) {
        let rooms: Vec<&GroupState> = self.st.groups.values().filter(|g| g.kind == Kind::Channel && !g.removed).collect();
        let msgs = rooms.iter().map(|g| self.st.chans.get(&g.gid).map_or(0, |l| l.msgs.iter().filter(|m| m.status != "sending").count())).sum();
        (rooms.iter().filter(|g| self.st.chans.get(&g.gid).is_some_and(|l| !l.msgs.is_empty())).count(), msgs)
    }

    /// Every room's messages in reading order, newest kept first when the whole does not fit. Returns the slices and how many
    /// messages were left out.
    fn history_export(&self) -> (Vec<Slice>, u64) {
        let mut all: Vec<(String, String, Msg, usize)> = vec![];
        for g in self.st.groups.values().filter(|g| g.kind == Kind::Channel && !g.removed) {
            for m in self.message_log(&g.gid) {
                if m.status == "sending" { continue; }
                let size = serde_json::to_vec(m).map(|v| v.len()).unwrap_or(0) + 8;
                all.push((g.gid.clone(), g.community.clone(), m.clone(), size));
            }
        }
        // newest first until the cap; what does not fit is the oldest, whichever room it is in
        let mut order: Vec<usize> = (0..all.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse((all[i].2.ts, all[i].2.seq)));
        let mut keep = vec![false; all.len()];
        let mut used = 0usize;
        let mut omitted = 0u64;
        for i in order { if used + all[i].3 <= TOTAL_CAP { used += all[i].3; keep[i] = true; } else { omitted += 1; } }
        let mut by_room: BTreeMap<String, Slice> = BTreeMap::new();
        for (i, (gid, community, m, _)) in all.into_iter().enumerate() {
            if !keep[i] { continue; }
            by_room.entry(gid.clone()).or_insert_with(|| Slice { gid: gid.clone(), community, unread: self.unread(&gid), msgs: vec![] }).msgs.push(m);
        }
        (by_room.into_values().collect(), omitted)
    }

    /// Send this device's history to a device that asked. Builds every chunk now (sealed to that device's key, signed by ours) and
    /// queues them; the rounds hand them to the inbox a few at a time.
    pub fn history_send(&mut self, device: &str) -> Result<Value> {
        let req = self.st.history_requests.get(device).cloned().ok_or_else(|| anyhow!("that device has not asked for the history"))?;
        self.own_other_device(&req.cert)?;
        let (slices, omitted) = self.history_export();
        let tid = rand_hex(16);
        // cut into chunks of at most CHUNK_PLAIN of plaintext: a slice is split where it has to be
        let limit = self.history_chunk_bytes.max(16 * 1024);
        let mut chunks: Vec<Vec<Slice>> = vec![vec![]];
        let mut cur = 0usize;
        for s in slices {
            let mut part = Slice { gid: s.gid.clone(), community: s.community.clone(), unread: s.unread, msgs: vec![] };
            for m in s.msgs {
                let size = serde_json::to_vec(&m).map(|v| v.len()).unwrap_or(0) + 8;
                if cur + size > limit && !(cur == 0 && part.msgs.is_empty()) {
                    if !part.msgs.is_empty() { chunks.last_mut().unwrap().push(std::mem::replace(&mut part, Slice { gid: s.gid.clone(), community: s.community.clone(), unread: s.unread, msgs: vec![] })); }
                    chunks.push(vec![]);
                    cur = 0;
                }
                cur += size;
                part.msgs.push(m);
            }
            if !part.msgs.is_empty() { chunks.last_mut().unwrap().push(part); cur += 64; }
        }
        if chunks.last().is_some_and(|c| c.is_empty()) && chunks.len() > 1 { chunks.pop(); }
        let n = chunks.len() as u32;
        let (k, cert) = (self.dev_key()?, self.cert()?);
        let mut out = vec![];
        for (i, slices) in chunks.into_iter().enumerate() {
            let i = i as u32;
            let plain = serde_json::to_vec(&ChunkPlain { v: 1, tid: tid.clone(), i, n, omitted, slices })?;
            let blob = crate::seal::seal_to(device, &chunk_aad(&tid, i, n, device), &pad(plain))?;
            let sig = hex::encode(k.sign(&chunk_msg(&tid, i, n, &blob)).to_bytes());
            let wire = serde_json::to_vec(&ChunkWire { tid: tid.clone(), i, n, cert: cert.clone(), blob: b64(&blob), sig })?;
            let mut item = MAGIC.to_vec(); item.extend(wire);
            out.push(HistoryOut { id: rand_hex(16), device: device.to_string(), tid: tid.clone(), i, n, data: b64(&item), queued: false });
        }
        let total = out.len();
        self.st.history_out.extend(out);
        self.st.history_requests.remove(device);
        self.st.history_sent.insert(device.to_string(), HistorySent { tid: tid.clone(), ts: now_secs(), chunks: total as u32, done: 0 });
        self.activity += 1;
        self.save()?;
        Ok(json!({"tid": tid, "chunks": total, "omitted": omitted}))
    }

    /// Leave a request unanswered (the new device shows that nobody answered).
    pub fn history_decline(&mut self, device: &str) -> Result<()> {
        self.st.history_requests.remove(device);
        self.activity += 1;
        self.save()
    }

    /// Hand the next chunks to the inbox queue, a few at a time, oldest transfer first. Runs every round.
    pub(crate) fn history_feed(&mut self, now: u64) {
        if self.st.history_out.is_empty() || now < self.history_hold_until { return; }
        let in_flight = self.st.welcome_out.iter().filter(|w| w.gid.starts_with(GID_PREFIX)).count();
        if in_flight >= IN_FLIGHT { return; }
        let revoked = self.st.revoked.clone();
        self.st.history_out.retain(|h| !revoked.contains_key(&h.device));
        let mut add = vec![];
        for h in self.st.history_out.iter_mut().filter(|h| !h.queued).take(IN_FLIGHT - in_flight) {
            h.queued = true;
            add.push(WelcomeOut { id: h.id.clone(), device: h.device.clone(), data: h.data.clone(), gid: format!("{GID_PREFIX}{}", h.tid) });
        }
        self.st.welcome_out.extend(add);
    }

    /// The outcome of posting one history item (the rounds call this for items whose group starts with `history:`).
    pub(crate) fn history_posted(&mut self, id: &str, r: &Result<()>) {
        let Some(h) = self.st.history_out.iter().find(|h| h.id == id).cloned() else { return };
        match r {
            Ok(()) => {
                self.st.history_out.retain(|x| x.id != id);
                if let Some(s) = self.st.history_sent.get_mut(&h.device) { s.done += 1; }
                self.activity += 1;
            }
            Err(e) if relay_status(e) == Some(404) => {
                // that device is gone (revoked): nothing more goes to it
                self.st.history_out.retain(|x| x.device != h.device);
                self.st.welcome_out.retain(|w| !(w.device == h.device && w.gid.starts_with(GID_PREFIX)));
                self.st.history_sent.remove(&h.device);
                self.notice("history", format!("stopped sending history to {}...: the relay no longer knows that device", &h.device[..8]));
            }
            Err(e) if relay_status(e) == Some(429) => { self.history_hold_until = now_secs() + 15; }
            Err(_) => {}
        }
    }

    /// For the UI: what is being received here, what is being sent from here, and who is asking.
    pub fn history_status(&self) -> Value {
        let now = now_secs();
        // the counts walk every room's log, so only when somebody is asking (the UI reads this with every snapshot)
        let (rooms, msgs) = if self.st.history_requests.is_empty() { (0, 0) } else { self.history_summary() };
        let incoming = match &self.st.history_in {
            Some(h) if !h.dismissed => json!({"state": if h.done > 0 { "done" } else { "receiving" }, "got": h.got.len(), "total": h.n, "msgs": h.msgs, "omitted": h.omitted, "from": h.from}),
            Some(_) => json!({"state": "none"}),
            None if self.st.history_wanted && self.st.history_asked > 0 && now >= self.st.history_wanted_since + 90 => json!({"state": "waiting"}),
            None if self.st.history_wanted => json!({"state": "asking"}),
            None => json!({"state": "none"}),
        };
        let requests: Vec<Value> = self.st.history_requests.iter().map(|(d, r)| json!({"device": d, "ts": r.ts, "created": r.cert.created})).collect();
        let sending: Vec<Value> = self.st.history_sent.iter().filter(|(d, s)| s.done < s.chunks || self.st.history_out.iter().any(|h| &h.device == *d) || now < s.ts + 120)
            .map(|(d, s)| json!({"device": d, "sent": s.done, "total": s.chunks, "done": s.done >= s.chunks && !self.st.history_out.iter().any(|h| &h.device == d)})).collect();
        json!({"incoming": incoming, "requests": requests, "sending": sending, "here": {"rooms": rooms, "msgs": msgs}})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn padding_hides_the_size_and_round_trips() {
        for n in [0usize, 1, 100, 4091, 4092, 4093, 10_000] {
            let v: Vec<u8> = (0..n).map(|i| i as u8).collect();
            let p = pad(v.clone());
            assert_eq!(p.len() % PAD, 0);
            assert_eq!(unpad(&p).unwrap(), &v[..]);
        }
        assert!(unpad(&[0, 0, 0, 9, 1]).is_none(), "a length past the end is refused");
    }
}
