#![allow(dead_code)]
use anyhow::Result;
use commons_core::client::Client;
use commons_core::transport::*;
use commons_core::vault::KdfParams;
use commons_proto::*;
use ed25519_dalek::SigningKey;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
pub struct Faults {
    /// reverse the order of entries in each sync response
    pub reorder: bool,
    /// duplicate every entry in each sync response
    pub duplicate: bool,
    /// execute the next N POSTs but report a network error to the caller (response lost)
    pub lose_post_responses: u32,
    /// drop the middle entry of each group's batch (relay withholds data)
    pub drop_middle: bool,
    /// fail every request (client is offline)
    pub offline: bool,
    /// sync returns nothing new (relay withholds; client acts on stale state)
    pub stale_sync: bool,
    /// cut sync responses at this many bytes, like a proxy or a connection dropping mid-way
    pub cut_sync_at: Option<usize>,
    /// the relay answers every signed request as if this computer's clock were wrong
    pub skewed_clock: bool,
    /// the relay is one from before the live channel: its paths answer 404
    pub no_live: bool,
    /// how many sync requests and live polls went out (whatever came of them)
    pub syncs: u32,
    pub live_polls: u32,
}

pub struct FlakyTransport { inner: HttpTransport, pub faults: Arc<Mutex<Faults>> }
impl Transport for FlakyTransport {
    fn request(&self, key: Option<&SigningKey>, method: &str, path: &str, body: &[u8], timeout: Duration) -> Result<HttpResp> {
        {
            let mut f = self.faults.lock().unwrap();
            if path == "/v1/sync" { f.syncs += 1; }
            if path == "/v1/live/poll" { f.live_polls += 1; }
            if f.offline { anyhow::bail!("network: simulated offline"); }
            if f.no_live && (path.ends_with("/live") || path == "/v1/live/poll") { return Ok(HttpResp { status: 404, body: b"not found".to_vec() }); }
            if f.skewed_clock && key.is_some() { return Ok(HttpResp { status: 401, body: format!("clock skew: relay time is {}", now_secs() + 3000).into_bytes() }); }
        }
        let mut r = self.inner.request(key, method, path, body, timeout)?;
        let mut f = self.faults.lock().unwrap();
        if method == "POST" && f.lose_post_responses > 0 && path.contains("/messages") {
            f.lose_post_responses -= 1;
            anyhow::bail!("network: simulated lost response");
        }
        if let Some(n) = f.cut_sync_at { if path == "/v1/sync" && r.body.len() > n { r.body.truncate(n); } }
        if path == "/v1/sync" && r.status == 200 && f.stale_sync {
            let s = SyncResp::default();
            r.body = serde_json::to_vec(&s)?;
        }
        if path == "/v1/sync" && r.status == 200 && (f.reorder || f.duplicate || f.drop_middle) {
            let mut s: SyncResp = serde_json::from_slice(&r.body)?;
            for v in s.messages.values_mut() {
                if f.drop_middle && v.len() >= 3 { let m = v.len() / 2; v.remove(m); }
                if f.duplicate { let c = v.clone(); v.extend(c); }
                if f.reorder { v.reverse(); }
            }
            r.body = serde_json::to_vec(&s)?;
        }
        Ok(r)
    }
}

pub struct Net {
    pub rt: tokio::runtime::Runtime,
    pub relay: Option<commons_relay::RelayHandle>,
    pub data: tempfile::TempDir,
    pub addr: std::net::SocketAddr,
    limits: commons_relay::Limits,
}
impl Net {
    pub fn new() -> Net { Self::with_limits(Default::default()) }
    pub fn with_limits(limits: commons_relay::Limits) -> Net {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let data = tempfile::tempdir().unwrap();
        let relay = rt.block_on(commons_relay::start_with("127.0.0.1:0".parse().unwrap(), data.path().join("relay"), limits.clone())).unwrap();
        let addr = relay.addr;
        assert!(addr.ip().is_loopback());
        Net { rt, relay: Some(relay), data, addr, limits }
    }
    pub fn url(&self) -> String { format!("http://{}", self.addr) }
    pub fn stop_relay(&mut self) { if let Some(r) = self.relay.take() { self.rt.block_on(r.stop()); } }
    pub fn start_relay(&mut self) {
        self.relay = Some(self.rt.block_on(commons_relay::start_with(self.addr, self.data.path().join("relay"), self.limits.clone())).unwrap());
    }
    pub fn db_path(&self) -> std::path::PathBuf { self.data.path().join("relay").join("relay.db") }
}

pub struct Peer { pub name: String, pub c: Client, pub dir: tempfile::TempDir, pub faults: Arc<Mutex<Faults>> }

pub fn transport(url: &str) -> (Arc<dyn Transport>, Arc<Mutex<Faults>>) {
    let faults = Arc::new(Mutex::new(Faults::default()));
    (Arc::new(FlakyTransport { inner: HttpTransport::new(url), faults: faults.clone() }), faults)
}

/// Tests do not wait 30 seconds for a device list to go stale.
pub fn impatient(c: &mut Client) { c.dir_ttl_secs = 0; c.acl_check_secs = 0; }

impl Peer {
    pub fn new(net: &Net, name: &str) -> Peer {
        let dir = tempfile::tempdir().unwrap();
        let (t, faults) = transport(&net.url());
        let mut c = Client::create(dir.path(), "pw", &net.url(), name, KdfParams::TEST, t).unwrap();
        impatient(&mut c);
        Peer { name: name.into(), c, dir, faults }
    }
    pub fn acct(&self) -> String { self.c.my_account() }
    pub fn tick(&mut self) { let _ = self.c.tick(0); }
    /// Re-open from disk (simulates app restart), optionally with a different state dir (restore).
    pub fn reopen(&mut self, net: &Net) {
        let (t, faults) = transport(&net.url());
        self.c.debug_release_lock();
        self.c = Client::open(self.dir.path(), "pw", t).unwrap();
        impatient(&mut self.c);
        self.faults = faults;
    }
}

/// Tick everyone until nothing happens any more.
pub fn settle(peers: &mut [&mut Peer]) {
    let mut quiet = 0;
    for _ in 0..60 {
        let before: Vec<u64> = peers.iter().map(|p| p.c.activity).collect();
        for p in peers.iter_mut() { p.tick(); }
        let after: Vec<u64> = peers.iter().map(|p| p.c.activity).collect();
        if before == after { quiet += 1; if quiet >= 2 { return; } } else { quiet = 0; }
    }
    panic!("did not settle; recent notices: {:?}", peers.iter().map(|p| (p.name.clone(), p.c.st.notices.iter().rev().take(6).map(|n| n.text.clone()).collect::<Vec<_>>())).collect::<Vec<_>>());
}

pub fn texts(p: &Peer, gid: &str) -> Vec<String> {
    p.c.messages(gid).into_iter().filter_map(|m| match m.body { commons_core::state::MsgBody::Text(t) if !m.deleted => Some(t), _ => None }).collect()
}
pub fn has_text(p: &Peer, gid: &str, t: &str) -> bool { texts(p, gid).iter().any(|x| x == t) }
pub fn notices(p: &Peer) -> Vec<String> { p.c.st.notices.iter().map(|n| format!("{}: {}", n.kind, n.text)).collect() }

/// Alice owns a community "club" with channel "general"; others join by invite link.
pub fn community_with(net: &Net, alice: &mut Peer, others: &mut [&mut Peer]) -> (String, String) {
    let cid = alice.c.create_community("club").unwrap();
    let gen = alice.c.channels_of(&cid)[0].gid.clone();
    for i in 0..others.len() {
        let link = alice.c.create_invite(&cid, None, 3600).unwrap();
        others[i].c.join_with_link(&link).unwrap();
        let mut all: Vec<&mut Peer> = vec![&mut *alice];
        for o in others.iter_mut() { all.push(&mut **o); }
        settle(&mut all);
    }
    let _ = net;
    (cid, gen)
}

pub fn relay_entries(net: &Net, gid: &str) -> Vec<(u64, Vec<u8>)> {
    let c = rusqlite::Connection::open_with_flags(net.db_path(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut s = c.prepare("SELECT seq,data FROM entries WHERE gid=?1 ORDER BY seq").unwrap();
    s.query_map([gid], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(|x| x.unwrap()).collect()
}
/// All bytes the relay holds on disk (db, wal, blobs).
pub fn relay_bytes(net: &Net) -> Vec<u8> {
    let mut all = vec![];
    fn walk(p: &std::path::Path, out: &mut Vec<u8>) {
        for e in std::fs::read_dir(p).unwrap() { let e = e.unwrap(); if e.path().is_dir() { walk(&e.path(), out) } else { out.extend(std::fs::read(e.path()).unwrap_or_default()) } }
    }
    walk(&net.data.path().join("relay"), &mut all);
    all
}
pub fn contains(hay: &[u8], needle: &str) -> bool { hay.windows(needle.len()).any(|w| w == needle.as_bytes()) }
