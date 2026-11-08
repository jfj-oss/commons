#![allow(dead_code)]
//! Test furniture: a throwaway relay (as crates/core/tests use) and a tiny HTTP server that stands in for Ollama and
//! DeepSeek. The real services are never contacted.
use commons_bot::gpu::{GpuCheck, Pause};
use commons_bot::search::{SearchResult, WebSearch};
use commons_bot::studio::{JobState, State, StudioApi};
use commons_core::client::Client;
use commons_core::state::MsgBody;
use commons_core::transport::HttpTransport;
use commons_core::vault::KdfParams;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------- fake model server

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: String,
}
impl Request {
    pub fn json(&self) -> serde_json::Value { serde_json::from_str(&self.body).unwrap_or_default() }
    pub fn header(&self, name: &str) -> Option<&str> { self.headers.get(&name.to_ascii_lowercase()).map(String::as_str) }
}

type Responder = dyn Fn(&Request) -> (u16, String) + Send + Sync;

pub struct FakeHttp {
    pub addr: SocketAddr,
    pub requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl FakeHttp {
    pub fn start(respond: impl Fn(&Request) -> (u16, String) + Send + Sync + 'static) -> FakeHttp {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let responder: Arc<Responder> = Arc::new(respond);
        let (s2, reqs, rsp) = (stop.clone(), requests.clone(), responder.clone());
        let join = std::thread::spawn(move || {
            while !s2.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((s, _)) => {
                        let (reqs, rsp) = (reqs.clone(), rsp.clone());
                        std::thread::spawn(move || handle(s, &*rsp, &reqs));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(5)),
                    Err(_) => break,
                }
            }
        });
        FakeHttp { addr, requests, stop, join: Some(join) }
    }
    pub fn url(&self) -> String { format!("http://{}", self.addr) }
    pub fn requests(&self) -> Vec<Request> { self.requests.lock().unwrap().clone() }
    pub fn hits(&self, path: &str) -> usize { self.requests.lock().unwrap().iter().filter(|r| r.path == path).count() }
}
impl Drop for FakeHttp {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() { let _ = j.join(); }
    }
}

fn handle(mut s: TcpStream, respond: &Responder, requests: &Mutex<Vec<Request>>) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(20)));
    let mut r = BufReader::new(match s.try_clone() { Ok(x) => x, Err(_) => return });
    let mut line = String::new();
    if r.read_line(&mut line).unwrap_or(0) == 0 { return; }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let mut headers = HashMap::new();
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" || h == "\n" { break; }
        if let Some((k, v)) = h.split_once(':') { headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string()); }
    }
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut body = vec![0u8; len];
    if len > 0 && r.read_exact(&mut body).is_err() { return; }
    let req = Request { method, path, headers, body: String::from_utf8_lossy(&body).into_owned() };
    requests.lock().unwrap().push(req.clone());
    let (code, body) = respond(&req);
    let head = format!("HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    let _ = s.write_all(head.as_bytes()).and_then(|_| s.write_all(body.as_bytes()));
    let _ = s.flush();
}

// ---------------------------------------------------------------- relay + peers

pub struct Net {
    pub rt: tokio::runtime::Runtime,
    pub relay: Option<commons_relay::RelayHandle>,
    pub data: tempfile::TempDir,
    pub addr: SocketAddr,
    limits: commons_relay::Limits,
}
impl Net {
    pub fn new() -> Net {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let data = tempfile::tempdir().unwrap();
        let limits = commons_relay::Limits::default();
        let relay = rt.block_on(commons_relay::start_with("127.0.0.1:0".parse().unwrap(), data.path().join("relay"), limits.clone())).unwrap();
        let addr = relay.addr;
        assert!(addr.ip().is_loopback());
        Net { rt, relay: Some(relay), data, addr, limits }
    }
    pub fn url(&self) -> String { format!("http://{}", self.addr) }
    pub fn db_path(&self) -> std::path::PathBuf { self.data.path().join("relay").join("relay.db") }
    pub fn stop_relay(&mut self) { if let Some(r) = self.relay.take() { self.rt.block_on(r.stop()); } }
    pub fn start_relay(&mut self) {
        self.relay = Some(self.rt.block_on(commons_relay::start_with(self.addr, self.data.path().join("relay"), self.limits.clone())).unwrap());
    }
}

pub struct Peer { pub name: String, pub c: Client, pub dir: tempfile::TempDir }

impl Peer {
    pub fn new(net: &Net, name: &str) -> Peer {
        let dir = tempfile::tempdir().unwrap();
        let mut c = Client::create(dir.path(), "pw", &net.url(), name, KdfParams::TEST, HttpTransport::new(&net.url()).into_arc()).unwrap();
        impatient(&mut c);
        Peer { name: name.into(), c, dir }
    }
    pub fn acct(&self) -> String { self.c.my_account() }
    pub fn tick(&mut self) { let _ = self.c.tick(0); }
}

pub fn impatient(c: &mut Client) { c.dir_ttl_secs = 0; c.acl_check_secs = 0; }

/// Tick everyone until two rounds in a row change nothing.
pub fn settle(peers: &mut [&mut Peer]) {
    let mut quiet = 0;
    for _ in 0..80 {
        let before: Vec<u64> = peers.iter().map(|p| p.c.activity).collect();
        for p in peers.iter_mut() { p.tick(); }
        let after: Vec<u64> = peers.iter().map(|p| p.c.activity).collect();
        if before == after { quiet += 1; if quiet >= 2 { return; } } else { quiet = 0; }
    }
    panic!("did not settle; notices: {:?}", peers.iter().map(|p| (p.name.clone(), p.c.st.notices.iter().rev().take(6).map(|n| n.text.clone()).collect::<Vec<_>>())).collect::<Vec<_>>());
}

pub fn texts(p: &Peer, gid: &str) -> Vec<String> {
    p.c.messages(gid).into_iter().filter_map(|m| match m.body { MsgBody::Text(t) if !m.deleted => Some(t), _ => None }).collect()
}
pub fn has_text(p: &Peer, gid: &str, t: &str) -> bool { texts(p, gid).iter().any(|x| x == t) }

/// Alice owns a community "club" with channel "general"; the others join by invite link.
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

pub fn relay_bytes(net: &Net) -> Vec<u8> {
    let mut all = vec![];
    fn walk(p: &std::path::Path, out: &mut Vec<u8>) {
        for e in std::fs::read_dir(p).unwrap() { let e = e.unwrap(); if e.path().is_dir() { walk(&e.path(), out) } else { out.extend(std::fs::read(e.path()).unwrap_or_default()) } }
    }
    walk(&net.data.path().join("relay"), &mut all);
    all
}
pub fn contains(hay: &[u8], needle: &str) -> bool { hay.windows(needle.len()).any(|w| w == needle.as_bytes()) }

/// A fake Ollama that answers with `answer` and reports the models in `models`. The model advertises tool support,
/// so the ordinary bot uses function calling; nothing ever calls the tool unless the answer says so.
pub fn fake_ollama(models: &[&str], answer: &str) -> FakeHttp {
    let models: Vec<String> = models.iter().map(|s| s.to_string()).collect();
    let answer = answer.to_string();
    FakeHttp::start(move |r| match r.path.as_str() {
        "/api/tags" => (200, serde_json::json!({"models": models.iter().map(|n| serde_json::json!({"name": n})).collect::<Vec<_>>()}).to_string()),
        "/api/show" => (200, serde_json::json!({"capabilities": ["completion", "tools"]}).to_string()),
        "/api/chat" => (200, serde_json::json!({"model": models.first().cloned().unwrap_or_default(), "message": {"role": "assistant", "content": answer}, "done": true}).to_string()),
        "/api/generate" => (200, serde_json::json!({"done": true}).to_string()),
        _ => (404, "{\"error\":\"not found\"}".into()),
    })
}

/// A fake Ollama that does not advertise tool support; the bot must fall back to the text protocol.
pub fn fake_ollama_without_tools(models: &[&str], answer: &str) -> FakeHttp {
    let models: Vec<String> = models.iter().map(|s| s.to_string()).collect();
    let answer = answer.to_string();
    FakeHttp::start(move |r| match r.path.as_str() {
        "/api/tags" => (200, serde_json::json!({"models": models.iter().map(|n| serde_json::json!({"name": n})).collect::<Vec<_>>()}).to_string()),
        "/api/show" => (200, serde_json::json!({"capabilities": ["completion"]}).to_string()),
        "/api/chat" => (200, serde_json::json!({"model": models.first().cloned().unwrap_or_default(), "message": {"role": "assistant", "content": answer}, "done": true}).to_string()),
        "/api/generate" => (200, serde_json::json!({"done": true}).to_string()),
        _ => (404, "{\"error\":\"not found\"}".into()),
    })
}

/// A fake DuckDuckGo HTML page server: every request gets `html`.
pub fn fake_search(html: &str) -> FakeHttp {
    let html = html.to_string();
    FakeHttp::start(move |_| (200, html.clone()))
}

/// A search backend that fails loudly: tests that must not search use it.
pub struct DeadSearch;
impl WebSearch for DeadSearch {
    fn search(&self, _query: &str) -> Result<Vec<SearchResult>, String> {
        Err("the test bot must not search".into())
    }
}

/// A GPU that is never busy: tests that are not about pausing use it.
pub struct NoGpu;
impl GpuCheck for NoGpu {
    fn blocked(&self) -> Option<Pause> { None }
}

/// A Studio that is never contacted: tests that are not about Studio use it.
pub struct DeadStudio;
impl StudioApi for DeadStudio {
    fn state(&self) -> Result<State, String> { Err("the test bot must not use Studio".into()) }
    fn submit(&self, _req: &commons_bot::studio::Request) -> Result<u64, String> { Err("the test bot must not use Studio".into()) }
    fn status(&self, _id: u64) -> Result<JobState, String> { Err("the test bot must not use Studio".into()) }
    fn download(&self, _kind: commons_bot::studio::Kind, _id: u64) -> Result<Vec<u8>, String> {
        Err("the test bot must not use Studio".into())
    }
}

/// A fake DeepSeek that answers every completion with `answer`.
pub fn fake_deepseek(answer: &str) -> FakeHttp {
    let answer = answer.to_string();
    FakeHttp::start(move |r| match r.path.as_str() {
        "/chat/completions" => (200, serde_json::json!({"choices": [{"message": {"role": "assistant", "content": answer}, "finish_reason": "stop"}]}).to_string()),
        _ => (404, "{\"error\":\"not found\"}".into()),
    })
}

/// Wait until a condition holds, ticking the given peers. Used around threads only.
pub fn until(timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout { if f() { return true; } std::thread::sleep(Duration::from_millis(50)); }
    f()
}
