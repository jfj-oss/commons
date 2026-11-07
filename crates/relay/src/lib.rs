//! Commons relay: an append-only, per-group log of opaque bytes plus a key-package directory,
//! a welcome inbox, an encrypted blob store and a tiny rendezvous mailbox for device pairing.
//! It assumes it is hostile: it holds no key that decrypts anything. See docs/THREAT-MODEL.md
//! for the exact list of what it can observe.
use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{HeaderMap, Method, Request, Response, StatusCode},
    Router,
};
use commons_proto::*;
use rand::Rng;
use rusqlite::{params, Connection, OptionalExtension};
use std::{collections::HashMap, net::SocketAddr, path::PathBuf, sync::{Arc, Mutex}, time::Duration};
use tokio::sync::{watch, Semaphore};

/// Who may use this relay and how much of it. The defaults suit a few friends on one small server; every limit
/// is per account or per device, so one person cannot use up what the others need.
#[derive(Clone, Debug)]
pub struct Limits {
    /// Optional call support: when set, current members can fetch LiveKit join tokens (see `LiveKitCfg`).
    pub livekit: Option<LiveKitCfg>,
    /// Call tokens one device may fetch per window (reconnects need a few, loops should not).
    pub call_tokens_per_window: u32,
    /// When set, a new account needs this secret to register (a device of an existing account does not).
    pub invite_secret: Option<String>,
    pub max_accounts: u64,
    pub max_devices_per_account: u64,
    /// Log bytes one account may post in total. The log is append-only, so this is a lifetime figure.
    pub max_entry_bytes_per_account: u64,
    pub max_blob_bytes_per_account: u64,
    pub max_inbox_items: u64,
    pub max_inbox_bytes: u64,
    pub max_rendezvous_per_account: u64,
    pub max_rendezvous_total: u64,
    /// Length of the window the rate limits below count in.
    pub window_secs: u64,
    /// New accounts, whole relay. Failed invite secrets count here too, which also limits guessing.
    pub registrations_per_window: u32,
    /// Join requests from non-members.
    pub joins_per_device_per_window: u32,
    pub joins_per_group_per_window: u32,
    /// Key packages one account may take from one device, and in total.
    pub kp_takes_per_target_per_window: u32,
    pub kp_takes_per_window: u32,
    /// Welcomes one account may post.
    pub welcomes_per_window: u32,
}
impl Default for Limits {
    fn default() -> Self {
        Limits {
            invite_secret: None,
            livekit: None,
            call_tokens_per_window: 60,
            max_accounts: 1000,
            max_devices_per_account: 32,
            max_entry_bytes_per_account: 4 << 30,
            max_blob_bytes_per_account: 2 << 30,
            max_inbox_items: 500,
            max_inbox_bytes: 64 << 20,
            max_rendezvous_per_account: 20,
            max_rendezvous_total: 5000,
            window_secs: 600,
            registrations_per_window: 60,
            joins_per_device_per_window: 10,
            joins_per_group_per_window: 120,
            kp_takes_per_target_per_window: 200,
            kp_takes_per_window: 2000,
            welcomes_per_window: 2000,
        }
    }
}

/// Call support: the relay signs short-lived LiveKit join tokens for current members of a group.
/// It holds the LiveKit API secret, so it can mint tokens but sees no media and no keys.
#[derive(Clone)]
pub struct LiveKitCfg {
    pub api_key: String,
    pub api_secret: String,
    /// The websocket URL clients should connect to (wss://... in production).
    pub url: String,
    pub token_ttl_secs: u64,
}
impl std::fmt::Debug for LiveKitCfg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "LiveKitCfg {{ url: {:?}, api_key: {:?}, api_secret: <hidden> }}", self.url, self.api_key) }
}

#[derive(serde::Serialize)]
struct LkVideoGrant { room: String, #[serde(rename = "roomJoin")] room_join: bool, #[serde(rename = "canPublish")] can_publish: bool, #[serde(rename = "canSubscribe")] can_subscribe: bool, #[serde(rename = "canPublishData")] can_publish_data: bool }
#[derive(serde::Serialize)]
struct LkClaims { iss: String, sub: String, jti: String, nbf: u64, exp: u64, video: LkVideoGrant }

/// Mint a LiveKit access token (HS256 JWT, standard `jsonwebtoken` crate). Public so tests can verify the exact claims.
pub fn mint_livekit_token(cfg: &LiveKitCfg, identity: &str, room: &str, now: u64) -> Result<(String, u64), jsonwebtoken::errors::Error> {
    let exp = now + cfg.token_ttl_secs;
    let claims = LkClaims { iss: cfg.api_key.clone(), sub: identity.into(), jti: identity.into(), nbf: now.saturating_sub(30), exp,
        video: LkVideoGrant { room: room.into(), room_join: true, can_publish: true, can_subscribe: true, can_publish_data: false } };
    let tok = jsonwebtoken::encode(&jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256), &claims, &jsonwebtoken::EncodingKey::from_secret(cfg.api_secret.as_bytes()))?;
    Ok((tok, exp))
}

pub struct RelayState {
    db: Mutex<Connection>,
    blob_dir: PathBuf,
    tick: watch::Sender<u64>,
    limits: Limits,
    /// Fixed-window counters. Lost on restart, which only makes a limit briefly more generous.
    rates: Mutex<HashMap<String, (u64, u32)>>,
    /// A 25 MB body is held in memory while it is checked: only a few at a time.
    big_bodies: Arc<Semaphore>,
}

type Resp = Response<Body>;
fn status(code: StatusCode, msg: &str) -> Resp {
    Response::builder().status(code).header("content-type", "text/plain").body(Body::from(msg.to_string())).unwrap()
}
fn json<T: serde::Serialize>(v: &T) -> Resp {
    Response::builder().status(200).header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(v).unwrap())).unwrap()
}
fn is_hex(s: &str, n: usize) -> bool { s.len() == n && s.bytes().all(|c| c.is_ascii_hexdigit()) }
fn valid_gid(s: &str) -> bool { s.len() >= 8 && s.len() <= 128 && s.bytes().all(|c| c.is_ascii_hexdigit()) }

pub fn open_db(path: &str) -> rusqlite::Result<Connection> {
    let c = Connection::open(path)?;
    c.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
         CREATE TABLE IF NOT EXISTS devices(device TEXT PRIMARY KEY, account TEXT NOT NULL, cert TEXT NOT NULL, revoked_sig TEXT);
         CREATE INDEX IF NOT EXISTS dev_acc ON devices(account);
         CREATE TABLE IF NOT EXISTS keypackages(id INTEGER PRIMARY KEY AUTOINCREMENT, device TEXT NOT NULL, data BLOB NOT NULL, last_resort INTEGER NOT NULL DEFAULT 0);
         CREATE INDEX IF NOT EXISTS kp_dev ON keypackages(device);
         CREATE TABLE IF NOT EXISTS group_members(gid TEXT NOT NULL, account TEXT NOT NULL, PRIMARY KEY(gid,account));
         CREATE TABLE IF NOT EXISTS group_auth(gid TEXT PRIMARY KEY, root TEXT NOT NULL, owner TEXT, owner_seq INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE IF NOT EXISTS call_denied(gid TEXT NOT NULL, account TEXT NOT NULL, PRIMARY KEY(gid,account));
         CREATE TABLE IF NOT EXISTS acl_demoted(root TEXT NOT NULL, account TEXT NOT NULL, PRIMARY KEY(root,account));
         CREATE TABLE IF NOT EXISTS entries(gid TEXT NOT NULL, seq INTEGER NOT NULL, id TEXT NOT NULL, data BLOB NOT NULL, PRIMARY KEY(gid,seq), UNIQUE(gid,id));
         CREATE TABLE IF NOT EXISTS welcomes(device TEXT NOT NULL, id TEXT NOT NULL, data BLOB NOT NULL, PRIMARY KEY(device,id));
         CREATE TABLE IF NOT EXISTS rendezvous(rid TEXT NOT NULL, slot TEXT NOT NULL, data BLOB NOT NULL, created INTEGER NOT NULL, owner TEXT, PRIMARY KEY(rid,slot));
         CREATE TABLE IF NOT EXISTS blobs(id TEXT PRIMARY KEY, account TEXT NOT NULL, size INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS blob_acc ON blobs(account);
         CREATE TABLE IF NOT EXISTS usage(account TEXT PRIMARY KEY, entry_bytes INTEGER NOT NULL DEFAULT 0);",
    )?;
    Ok(c)
}

pub struct RelayHandle {
    pub addr: SocketAddr,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<()>>,
}
impl RelayHandle {
    pub fn url(&self) -> String { format!("http://{}", self.addr) }
    pub async fn stop(mut self) {
        if let Some(s) = self.shutdown.take() { let _ = s.send(()); }
        if let Some(j) = self.join.take() { let _ = j.await; }
    }
}

/// Start a relay. `bind` should be 127.0.0.1:port for local use.
pub async fn start(bind: SocketAddr, data_dir: PathBuf) -> std::io::Result<RelayHandle> {
    start_with(bind, data_dir, Limits::default()).await
}
pub async fn start_with(bind: SocketAddr, data_dir: PathBuf, limits: Limits) -> std::io::Result<RelayHandle> {
    std::fs::create_dir_all(data_dir.join("blobs"))?;
    let db = open_db(data_dir.join("relay.db").to_str().unwrap()).map_err(|e| std::io::Error::other(e.to_string()))?;
    let (tick, _) = watch::channel(0u64);
    let st = Arc::new(RelayState { db: Mutex::new(db), blob_dir: data_dir.join("blobs"), tick, limits, rates: Mutex::new(HashMap::new()), big_bodies: Arc::new(Semaphore::new(4)) });
    let app = Router::new().fallback(handle).with_state(st);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    let addr = listener.local_addr()?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let join = tokio::spawn(async move {
        let _ = axum::serve(listener, app).with_graceful_shutdown(async { let _ = rx.await; }).await;
    });
    Ok(RelayHandle { addr, shutdown: Some(tx), join: Some(join) })
}

struct Authed { device: String, account: String }

fn authenticate(db: &Connection, headers: &HeaderMap, method: &str, path: &str, body: &[u8], require_registered: bool) -> Result<(String, Option<Authed>), Resp> {
    let h = headers.get(AUTH_HEADER).and_then(|v| v.to_str().ok()).ok_or_else(|| status(StatusCode::UNAUTHORIZED, "missing auth"))?;
    let dev = verify_request(h, method, path, body).map_err(|e| match e {
        // Says so plainly, with our time, so that the client can tell its user to fix the clock.
        ProtoError::ClockSkew => status(StatusCode::UNAUTHORIZED, &format!("clock skew: relay time is {}", now_secs())),
        _ => status(StatusCode::UNAUTHORIZED, "bad auth"),
    })?;
    let row: Option<(String, Option<String>)> = db.query_row("SELECT account, revoked_sig FROM devices WHERE device=?1", [&dev], |r| Ok((r.get(0)?, r.get(1)?))).optional()
        .map_err(|_| status(StatusCode::INTERNAL_SERVER_ERROR, "db"))?;
    match row {
        Some((_, Some(_))) => Err(status(StatusCode::FORBIDDEN, "device revoked")),
        Some((account, None)) => Ok((dev.clone(), Some(Authed { device: dev, account }))),
        None if require_registered => Err(status(StatusCode::FORBIDDEN, "device not registered")),
        None => Ok((dev, None)),
    }
}

fn parse<T: serde::de::DeserializeOwned>(b: &[u8]) -> Result<T, Resp> {
    serde_json::from_slice(b).map_err(|_| status(StatusCode::BAD_REQUEST, "bad json"))
}
fn dec(s: &str, max: usize) -> Result<Vec<u8>, Resp> {
    if s.len() > max * 2 { return Err(status(StatusCode::PAYLOAD_TOO_LARGE, "too large")); }
    let v = unb64(s).map_err(|_| status(StatusCode::BAD_REQUEST, "bad base64"))?;
    if v.len() > max { return Err(status(StatusCode::PAYLOAD_TOO_LARGE, "too large")); }
    Ok(v)
}
fn dberr<E: std::fmt::Display>(e: E) -> Resp { tracing::error!("db error: {e}"); status(StatusCode::INTERNAL_SERVER_ERROR, "db") }

/// Two secrets of any length, compared without an early exit.
fn same_secret(a: &str, b: &str) -> bool {
    let (x, y) = (sha256(&[a.as_bytes()]), sha256(&[b.as_bytes()]));
    x.iter().zip(y.iter()).fold(0u8, |d, (p, q)| d | (p ^ q)) == 0
}

impl RelayState {
    /// Counts one use of `key` and says whether it stays within `limit` for the current window.
    fn allow(&self, key: String, limit: u32) -> bool {
        let w = now_secs() / self.limits.window_secs.max(1);
        let mut m = self.rates.lock().unwrap();
        if m.len() > 100_000 { m.retain(|_, v| v.0 == w); }
        let e = m.entry(key).or_insert((w, 0));
        if e.0 != w { *e = (w, 0); }
        if e.1 >= limit { return false; }
        e.1 += 1;
        true
    }
    fn limited(&self, key: String, limit: u32, what: &str) -> Result<(), Resp> {
        if self.allow(key, limit) { Ok(()) } else { Err(status(StatusCode::TOO_MANY_REQUESTS, what)) }
    }
}

/// Bodies are read before authentication (the signature covers them), so only blob uploads may be big.
fn body_limit(m: &Method, path: &str) -> usize {
    if *m == Method::PUT && path.starts_with("/v1/blobs/") { MAX_BLOB_BYTES + 1024 } else { 4 * 1024 * 1024 }
}

async fn handle(State(st): State<Arc<RelayState>>, req: Request<Body>) -> Resp {
    let (parts, body) = req.into_parts();
    let path = parts.uri.path().to_string();
    let m = parts.method.clone();
    let limit = body_limit(&m, &path);
    let _permit = if limit > 4 * 1024 * 1024 { st.big_bodies.clone().acquire_owned().await.ok() } else { None };
    let body = match to_bytes(body, limit).await { Ok(b) => b, Err(_) => return status(StatusCode::PAYLOAD_TOO_LARGE, "too large") };
    // The long-poll endpoint must not hold the DB lock while waiting.
    if m == Method::POST && path == "/v1/sync" {
        return sync(st, &parts.headers, &path, &body).await;
    }
    let seg: Vec<String> = path.trim_matches('/').split('/').map(String::from).collect();
    // Blob bytes go to and from the disk with no database lock held, and downloads are streamed.
    if seg.len() == 3 && seg[0] == "v1" && seg[1] == "blobs" {
        if m == Method::GET { return get_blob(st, &parts.headers, &path, &seg[2]).await; }
        if m == Method::PUT { return put_blob(st, parts.headers, path.clone(), seg[2].clone(), body).await; }
    }
    let st2 = st.clone();
    let headers = parts.headers.clone();
    // Everything else is short: run it on a blocking thread so the sqlite mutex never blocks the runtime.
    tokio::task::spawn_blocking(move || {
        let seg: Vec<&str> = seg.iter().map(String::as_str).collect();
        route(&st2, &m, m.as_str(), &seg, &headers, &path, &body).unwrap_or_else(|r| r)
    }).await.unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR, "panic"))
}

async fn get_blob(st: Arc<RelayState>, headers: &HeaderMap, path: &str, id: &str) -> Resp {
    { let db = st.db.lock().unwrap(); if let Err(r) = authenticate(&db, headers, "GET", path, &[], true) { return r; } }
    if !is_hex(id, 32) { return status(StatusCode::BAD_REQUEST, "bad id"); }
    let Ok(f) = tokio::fs::File::open(st.blob_dir.join(id)).await else { return status(StatusCode::NOT_FOUND, "no such blob") };
    let len = f.metadata().await.map(|m| m.len()).unwrap_or(0);
    Response::builder().status(200).header("content-type", "application/octet-stream").header("content-length", len)
        .body(Body::from_stream(tokio_util::io::ReaderStream::new(f))).unwrap()
}

async fn put_blob(st: Arc<RelayState>, headers: HeaderMap, path: String, id: String, body: axum::body::Bytes) -> Resp {
    let task = tokio::task::spawn_blocking(move || -> Result<Resp, Resp> {
        let account = {
            let db = st.db.lock().unwrap();
            let a = authenticate(&db, &headers, "PUT", &path, &body, true)?.1.unwrap();
            if !is_hex(&id, 32) { return Err(status(StatusCode::BAD_REQUEST, "bad id")); }
            if body.len() > MAX_BLOB_BYTES { return Err(status(StatusCode::PAYLOAD_TOO_LARGE, "too large")); }
            if st.blob_dir.join(&id).exists() { return Err(status(StatusCode::CONFLICT, "exists")); }
            let used: i64 = db.query_row("SELECT COALESCE(SUM(size),0) FROM blobs WHERE account=?1", [&a.account], |r| r.get(0)).map_err(dberr)?;
            if used as u64 + body.len() as u64 > st.limits.max_blob_bytes_per_account { return Err(status(StatusCode::INSUFFICIENT_STORAGE, "file storage quota reached")); }
            // Reserving the id first also stops two uploads racing for it.
            let n = db.execute("INSERT OR IGNORE INTO blobs(id,account,size) VALUES(?1,?2,?3)", params![id, a.account, body.len() as i64]).map_err(dberr)?;
            if n == 0 { return Err(status(StatusCode::CONFLICT, "exists")); }
            a.account
        };
        let (tmp, p) = (st.blob_dir.join(format!("{id}.tmp")), st.blob_dir.join(&id));
        if let Err(e) = std::fs::write(&tmp, &body).and_then(|_| std::fs::rename(&tmp, &p)) {
            let _ = std::fs::remove_file(&tmp);
            let db = st.db.lock().unwrap();
            let _ = db.execute("DELETE FROM blobs WHERE id=?1 AND account=?2", params![id, account]);
            return Err(dberr(e));
        }
        Ok(status(StatusCode::OK, "ok"))
    });
    match task.await { Ok(r) => r.unwrap_or_else(|r| r), Err(_) => status(StatusCode::INTERNAL_SERVER_ERROR, "panic") }
}

/// Where a group's authority lives: the row of its root (control) group.
struct GroupAuth { root: String, owner: String, seq: u64 }
fn group_auth(db: &Connection, gid: &str) -> Result<Option<GroupAuth>, Resp> {
    let root: Option<String> = db.query_row("SELECT root FROM group_auth WHERE gid=?1", [gid], |r| r.get(0)).optional().map_err(dberr)?;
    let Some(root) = root else { return Ok(None) };
    let row: Option<(Option<String>, u64)> = db.query_row("SELECT owner, owner_seq FROM group_auth WHERE gid=?1", [&root], |r| Ok((r.get(0)?, r.get(1)?))).optional().map_err(dberr)?;
    Ok(row.and_then(|(o, seq)| o.map(|owner| GroupAuth { root, owner, seq })))
}
fn is_member(db: &Connection, gid: &str, account: &str) -> Result<bool, Resp> {
    Ok(db.query_row("SELECT COUNT(*) FROM group_members WHERE gid=?1 AND account=?2", params![gid, account], |r| r.get::<_, i64>(0)).map_err(dberr)? > 0)
}
/// The owner, or an account whose owner-signed grant checks out and has not been withdrawn.
fn has_authority(db: &Connection, auth: &GroupAuth, account: &str, grant: Option<&str>) -> Result<bool, Resp> {
    if account == auth.owner { return Ok(true); }
    let Some(g) = grant else { return Ok(false) };
    if verify_grant(&auth.owner, &auth.root, account, g).is_err() { return Ok(false); }
    let gone: i64 = db.query_row("SELECT COUNT(*) FROM acl_demoted WHERE root=?1 AND account=?2", params![auth.root, account], |r| r.get(0)).map_err(dberr)?;
    Ok(gone == 0)
}

/// Change a group's ACL. The relay cannot read a group's table, so it checks the part of the group's own
/// authorisation it can be shown: who owns it, and that the requester holds a grant signed by that owner.
fn change_acl(db: &Connection, gid: &str, a: &Authed, r: &AclReq) -> Result<(), Resp> {
    let names = r.add.len() + r.remove.len() + r.demote.len() + r.promote.len() + r.call_remove.len();
    if names > MAX_ACL_BATCH { return Err(status(StatusCode::PAYLOAD_TOO_LARGE, "too many accounts in one request")); }
    if !r.add.iter().chain(&r.remove).chain(&r.demote).chain(&r.promote).chain(&r.call_remove).all(|x| is_hex(x, 64)) { return Err(status(StatusCode::BAD_REQUEST, "bad account")); }
    let mut auth = group_auth(db, gid)?;
    if auth.is_none() {
        // First claim of this group id. Control group ids are hashes nobody can predict and channel ids are
        // random, so nobody can claim a group ahead of its creator. Groups from before owners were recorded
        // can only be claimed from inside.
        let members: i64 = db.query_row("SELECT COUNT(*) FROM group_members WHERE gid=?1", [gid], |r| r.get(0)).map_err(dberr)?;
        if members > 0 && !is_member(db, gid, &a.account)? { return Err(status(StatusCode::FORBIDDEN, "not a member")); }
        match r.root.as_deref() {
            None => { db.execute("INSERT INTO group_auth(gid,root,owner) VALUES(?1,?1,?2)", params![gid, a.account]).map_err(dberr)?; }
            Some(root) if root == gid => { db.execute("INSERT INTO group_auth(gid,root,owner) VALUES(?1,?1,?2)", params![gid, a.account]).map_err(dberr)?; }
            Some(root) => {
                let Some(ra) = group_auth(db, root)? else { return Err(status(StatusCode::NOT_FOUND, "unknown community")) };
                if !has_authority(db, &ra, &a.account, r.grant.as_deref())? { return Err(status(StatusCode::FORBIDDEN, "only an admin can add a channel")); }
                db.execute("INSERT INTO group_auth(gid,root) VALUES(?1,?2)", params![gid, root]).map_err(dberr)?;
            }
        }
        db.execute("INSERT OR IGNORE INTO group_members(gid,account) VALUES(?1,?2)", params![gid, a.account]).map_err(dberr)?;
        auth = group_auth(db, gid)?;
    }
    let mut auth = auth.ok_or_else(|| status(StatusCode::INTERNAL_SERVER_ERROR, "group has no owner"))?;
    if let Some(t) = &r.transfer {
        if t.seq == auth.seq + 1 && t.from == auth.owner && is_hex(&t.to, 64) && verify_transfer(&auth.root, t.seq, &t.from, &t.to, &t.sig).is_ok() {
            db.execute("UPDATE group_auth SET owner=?1, owner_seq=?2 WHERE gid=?3", params![t.to, t.seq, auth.root]).map_err(dberr)?;
            auth.owner = t.to.clone(); auth.seq = t.seq;
        } else if t.seq > auth.seq {
            return Err(status(StatusCode::FORBIDDEN, "bad ownership transfer"));
        }
    }
    let member = is_member(db, gid, &a.account)?;
    let authority = has_authority(db, &auth, &a.account, r.grant.as_deref())?;
    if !member && !authority { return Err(status(StatusCode::FORBIDDEN, "not a member")); }
    if r.remove.contains(&auth.owner) { return Err(status(StatusCode::FORBIDDEN, "the owner cannot be removed")); }
    if (!r.demote.is_empty() || !r.promote.is_empty()) && a.account != auth.owner { return Err(status(StatusCode::FORBIDDEN, "only the owner can change admins")); }
    // Only a real change needs authority: a member adding a device of their own account names themselves.
    let mut real = false;
    for x in &r.add { real |= !is_member(db, gid, x)?; }
    for x in &r.remove { real |= is_member(db, gid, x)?; }
    // Cutting someone off from calls is a change to who is in the group like any other: owner or granted admin only.
    for x in &r.call_remove {
        if x == &auth.owner { return Err(status(StatusCode::FORBIDDEN, "the owner cannot be removed")); }
        let denied: i64 = db.query_row("SELECT COUNT(*) FROM call_denied WHERE gid=?1 AND account=?2", params![gid, x], |r| r.get(0)).map_err(dberr)?;
        real |= denied == 0;
    }
    if real && !authority { return Err(status(StatusCode::FORBIDDEN, "only an admin can change who is in a group")); }
    for x in &r.add {
        db.execute("INSERT OR IGNORE INTO group_members(gid,account) VALUES(?1,?2)", params![gid, x]).map_err(dberr)?;
        // only an admin's add lifts a call cut-off (a denied member re-adding their own account is not a "real" change)
        if authority { db.execute("DELETE FROM call_denied WHERE gid=?1 AND account=?2", params![gid, x]).map_err(dberr)?; }
    }
    for x in &r.call_remove { db.execute("INSERT OR IGNORE INTO call_denied(gid,account) VALUES(?1,?2)", params![gid, x]).map_err(dberr)?; }
    for x in &r.remove { db.execute("DELETE FROM group_members WHERE gid=?1 AND account=?2", params![gid, x]).map_err(dberr)?; }
    for x in &r.demote { db.execute("INSERT OR IGNORE INTO acl_demoted(root,account) VALUES(?1,?2)", params![auth.root, x]).map_err(dberr)?; }
    for x in &r.promote { db.execute("DELETE FROM acl_demoted WHERE root=?1 AND account=?2", params![auth.root, x]).map_err(dberr)?; }
    Ok(())
}

/// Runs `f` in one transaction, so a request is all-or-nothing and costs one disk sync instead of one per row.
fn atomically<T>(db: &Connection, f: impl FnOnce() -> Result<T, Resp>) -> Result<T, Resp> {
    let tx = db.unchecked_transaction().map_err(dberr)?;
    let v = f()?;
    tx.commit().map_err(dberr)?;
    Ok(v)
}

fn route(st: &RelayState, m: &Method, ms: &str, seg: &[&str], headers: &HeaderMap, path: &str, body: &[u8]) -> Result<Resp, Resp> {
    let lim = &st.limits;
    if *m == Method::GET && seg == ["health"] { return Ok(status(StatusCode::OK, "ok")); }
    let db = st.db.lock().unwrap();
    match (m.clone(), seg) {
        (Method::GET, ["v1", "info"]) => Ok(json(&InfoResp { gated: lim.invite_secret.is_some(), ok: false })),
        // Lets an app check a secret before it creates anything on disk. Counts against guessing.
        (Method::POST, ["v1", "info"]) => {
            let r: InfoReq = parse(body)?;
            let Some(secret) = &lim.invite_secret else { return Ok(json(&InfoResp { gated: false, ok: true })) };
            st.limited("register".into(), lim.registrations_per_window, "too many attempts, try again later")?;
            Ok(json(&InfoResp { gated: true, ok: same_secret(secret, &r.invite) }))
        }
        // ---- device registry ----
        (Method::POST, ["v1", "devices"]) => {
            let (dev, known) = authenticate(&db, headers, ms, path, body, false)?;
            let r: RegisterReq = parse(body)?;
            if r.cert.device != dev || r.cert.verify().is_err() { return Err(status(StatusCode::BAD_REQUEST, "bad cert")); }
            if known.is_some() { return Ok(status(StatusCode::OK, "ok")); }
            let devices: u64 = db.query_row("SELECT COUNT(*) FROM devices WHERE account=?1", [&r.cert.account], |r| r.get(0)).map_err(dberr)?;
            if devices == 0 {
                // A new account: this is where a relay decides who gets in. A cert signed by a known account
                // needs no secret: that is a second device, or a recovery, of someone already here.
                st.limited("register".into(), lim.registrations_per_window, "too many new accounts, try again later")?;
                if let Some(secret) = &lim.invite_secret {
                    if !r.invite.as_deref().is_some_and(|i| same_secret(secret, i)) { return Err(status(StatusCode::FORBIDDEN, "registration needs this relay's invite secret")); }
                }
                let accounts: u64 = db.query_row("SELECT COUNT(DISTINCT account) FROM devices", [], |r| r.get(0)).map_err(dberr)?;
                if accounts >= lim.max_accounts { return Err(status(StatusCode::FORBIDDEN, "this relay is full")); }
            } else if devices >= lim.max_devices_per_account {
                return Err(status(StatusCode::TOO_MANY_REQUESTS, "too many devices on this account"));
            }
            // A revoked device id can never come back (checked in authenticate for registered ones).
            db.execute("INSERT OR IGNORE INTO devices(device,account,cert) VALUES(?1,?2,?3)",
                params![r.cert.device, r.cert.account, serde_json::to_string(&r.cert).unwrap()]).map_err(dberr)?;
            Ok(status(StatusCode::OK, "ok"))
        }
        (Method::POST, ["v1", "devices", "revoke"]) => {
            let r: RevokeReq = parse(body)?;
            verify_revocation(&r.account, &r.device, &r.sig).map_err(|_| status(StatusCode::BAD_REQUEST, "bad sig"))?;
            db.execute("UPDATE devices SET revoked_sig=?1 WHERE device=?2 AND account=?3", params![r.sig, r.device, r.account]).map_err(dberr)?;
            db.execute("DELETE FROM keypackages WHERE device=?1", [&r.device]).map_err(dberr)?;
            db.execute("DELETE FROM welcomes WHERE device=?1", [&r.device]).map_err(dberr)?;
            Ok(status(StatusCode::OK, "ok"))
        }
        (Method::GET, ["v1", "accounts", acc, "devices"]) => {
            authenticate(&db, headers, ms, path, body, true)?;
            if !is_hex(acc, 64) { return Err(status(StatusCode::BAD_REQUEST, "bad account")); }
            let mut s = db.prepare("SELECT cert, revoked_sig FROM devices WHERE account=?1").map_err(dberr)?;
            let rows = s.query_map([acc], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))).map_err(dberr)?;
            let mut devices = vec![];
            for r in rows { let (c, rs) = r.map_err(dberr)?; if let Ok(cert) = serde_json::from_str(&c) { devices.push(DeviceInfo { cert, revoked_sig: rs }); } }
            Ok(json(&DevicesResp { devices }))
        }
        // ---- key package directory ----
        (Method::POST, ["v1", "keypackages"]) => {
            let a = authenticate(&db, headers, ms, path, body, true)?.1.unwrap();
            let r: PublishKpReq = parse(body)?;
            if r.packages.len() > 200 { return Err(status(StatusCode::BAD_REQUEST, "too many")); }
            let have: i64 = db.query_row("SELECT COUNT(*) FROM keypackages WHERE device=?1 AND last_resort=0", [&a.device], |r| r.get(0)).map_err(dberr)?;
            if have + r.packages.len() as i64 > 500 { return Err(status(StatusCode::TOO_MANY_REQUESTS, "directory quota")); }
            atomically(&db, || {
                for p in &r.packages {
                    let d = dec(p, 8192)?;
                    db.execute("INSERT INTO keypackages(device,data,last_resort) VALUES(?1,?2,0)", params![a.device, d]).map_err(dberr)?;
                }
                if let Some(lr) = &r.last_resort {
                    let d = dec(lr, 8192)?;
                    db.execute("DELETE FROM keypackages WHERE device=?1 AND last_resort=1", [&a.device]).map_err(dberr)?;
                    db.execute("INSERT INTO keypackages(device,data,last_resort) VALUES(?1,?2,1)", params![a.device, d]).map_err(dberr)?;
                }
                Ok(())
            })?;
            Ok(status(StatusCode::OK, "ok"))
        }
        (Method::GET, ["v1", "keypackages", dev]) => {
            let a = authenticate(&db, headers, ms, path, body, true)?.1.unwrap();
            if !is_hex(dev, 64) { return Err(status(StatusCode::BAD_REQUEST, "bad device")); }
            let revoked: Option<Option<String>> = db.query_row("SELECT revoked_sig FROM devices WHERE device=?1", [dev], |r| r.get(0)).optional().map_err(dberr)?;
            if !matches!(revoked, Some(None)) { return Ok(json(&KpResp { package: None })); }
            // Taking a package consumes it: without a limit anyone could empty a stranger's supply.
            st.limited(format!("kp|{}|{dev}", a.account), lim.kp_takes_per_target_per_window, "too many key packages taken")?;
            st.limited(format!("kp|{}", a.account), lim.kp_takes_per_window, "too many key packages taken")?;
            let row: Option<(i64, Vec<u8>, i64)> = db.query_row(
                "SELECT id,data,last_resort FROM keypackages WHERE device=?1 ORDER BY last_resort ASC, id ASC LIMIT 1", [dev],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional().map_err(dberr)?;
            if let Some((id, data, lr)) = row {
                if lr == 0 { db.execute("DELETE FROM keypackages WHERE id=?1", [id]).map_err(dberr)?; }
                return Ok(json(&KpResp { package: Some(b64(&data)) }));
            }
            Ok(json(&KpResp { package: None }))
        }
        (Method::GET, ["v1", "keypackages", dev, "count"]) => {
            authenticate(&db, headers, ms, path, body, true)?;
            let n: i64 = db.query_row("SELECT COUNT(*) FROM keypackages WHERE device=?1 AND last_resort=0", [dev], |r| r.get(0)).map_err(dberr)?;
            Ok(status(StatusCode::OK, &n.to_string()))
        }
        // ---- groups: ACL (by account), append-only log ----
        (Method::POST, ["v1", "groups", gid, "acl"]) => {
            let a = authenticate(&db, headers, ms, path, body, true)?.1.unwrap();
            if !valid_gid(gid) { return Err(status(StatusCode::BAD_REQUEST, "bad group")); }
            let r: AclReq = parse(body)?;
            atomically(&db, || change_acl(&db, gid, &a, &r))?;
            Ok(status(StatusCode::OK, "ok"))
        }
        (Method::GET, ["v1", "groups", gid, "acl"]) => {
            let a = authenticate(&db, headers, ms, path, body, true)?.1.unwrap();
            if !valid_gid(gid) { return Err(status(StatusCode::BAD_REQUEST, "bad group")); }
            // Unknown here means the relay lost it. Claiming it afresh would restart its log below everyone's cursor and
            // new entries would go unseen, so clients must be told apart from "not a member".
            if !group_exists(&db, gid) { return Err(status(StatusCode::NOT_FOUND, "no such group")); }
            let auth = group_auth(&db, gid)?;
            let owner = auth.as_ref().is_some_and(|g| g.owner == a.account);
            if !owner && !is_member(&db, gid, &a.account)? { return Err(status(StatusCode::FORBIDDEN, "not a member")); }
            let mut s = db.prepare("SELECT account FROM group_members WHERE gid=?1").map_err(dberr)?;
            let members = s.query_map([gid], |r| r.get::<_, String>(0)).map_err(dberr)?.collect::<Result<Vec<_>, _>>().map_err(dberr)?;
            let (owner, owner_seq) = auth.map(|g| (g.owner, g.seq)).unwrap_or_default();
            Ok(json(&AclResp { owner, owner_seq, members }))
        }
        // ---- calls: a short-lived LiveKit join token for a current member. The media server is untrusted and never sees
        // the group id (the room name is a one-way hash of it); the media key comes from MLS, not from here.
        (Method::POST, ["v1", "groups", gid, "call-token"]) => {
            let a = authenticate(&db, headers, ms, path, body, true)?.1.unwrap();
            let Some(cfg) = lim.livekit.as_ref() else { return Err(status(StatusCode::NOT_FOUND, "this relay has no call server configured")) };
            if !valid_gid(gid) { return Err(status(StatusCode::BAD_REQUEST, "bad group")); }
            st.limited(format!("call:{}", a.device), lim.call_tokens_per_window, "too many call tokens, wait a little")?;
            let member = is_member(&db, gid, &a.account)?;
            let denied: i64 = db.query_row("SELECT COUNT(*) FROM call_denied WHERE gid=?1 AND account=?2", params![gid, a.account], |r| r.get(0)).map_err(dberr)?;
            if !member || denied > 0 { return Err(status(StatusCode::FORBIDDEN, "not a current member")); }
            let room = call_room(gid);
            let (token, expires) = mint_livekit_token(cfg, &a.device, &room, now_secs()).map_err(|_| status(StatusCode::INTERNAL_SERVER_ERROR, "token"))?;
            Ok(json(&CallTokenResp { url: cfg.url.clone(), token, room, identity: a.device, expires }))
        }
        (Method::POST, ["v1", "groups", gid, "messages"]) => {
            let a = authenticate(&db, headers, ms, path, body, true)?.1.unwrap();
            let r: PostReq = parse(body)?;
            if !is_hex(&r.id, 32) { return Err(status(StatusCode::BAD_REQUEST, "bad id")); }
            let data = dec(&r.data, MAX_MESSAGE_BYTES)?;
            if data.is_empty() { return Err(status(StatusCode::BAD_REQUEST, "empty")); }
            let member = is_member(&db, gid, &a.account)?;
            // Join requests come from devices that are not yet members.
            let is_join = data[0] == TAG_JOIN_REQUEST;
            if !member && !(is_join && group_exists(&db, gid)) { return Err(status(StatusCode::FORBIDDEN, "not a member")); }
            if is_join && data.len() > 4096 { return Err(status(StatusCode::PAYLOAD_TOO_LARGE, "too large")); }
            if let Some(seq) = db.query_row("SELECT seq FROM entries WHERE gid=?1 AND id=?2", params![gid, r.id], |r| r.get::<_, u64>(0)).optional().map_err(dberr)? {
                return Ok(json(&PostResp { seq, duplicate: true }));
            }
            if !member {
                // Anyone registered can knock, so knocking is limited; the limit is per device first, so that one
                // noisy device does not use up the group's allowance by itself.
                st.limited(format!("join|{}|{gid}", a.device), lim.joins_per_device_per_window, "too many join requests from this device")?;
                st.limited(format!("join|{gid}"), lim.joins_per_group_per_window, "too many join requests for this community")?;
            }
            let used: u64 = db.query_row("SELECT entry_bytes FROM usage WHERE account=?1", [&a.account], |r| r.get(0)).optional().map_err(dberr)?.unwrap_or(0);
            if used + data.len() as u64 > lim.max_entry_bytes_per_account { return Err(status(StatusCode::INSUFFICIENT_STORAGE, "message storage quota reached")); }
            let seq: u64 = db.query_row("SELECT COALESCE(MAX(seq),0)+1 FROM entries WHERE gid=?1", [gid], |r| r.get(0)).map_err(dberr)?;
            atomically(&db, || {
                db.execute("INSERT INTO entries(gid,seq,id,data) VALUES(?1,?2,?3,?4)", params![gid, seq, r.id, data]).map_err(dberr)?;
                db.execute("INSERT INTO usage(account,entry_bytes) VALUES(?1,?2) ON CONFLICT(account) DO UPDATE SET entry_bytes=entry_bytes+?2", params![a.account, data.len() as i64]).map_err(dberr)?;
                Ok(())
            })?;
            st.tick.send_modify(|t| *t += 1);
            Ok(json(&PostResp { seq, duplicate: false }))
        }
        // ---- welcome inbox (per device) ----
        (Method::POST, ["v1", "welcomes", dev]) => {
            let a = authenticate(&db, headers, ms, path, body, true)?.1.unwrap();
            if !is_hex(dev, 64) { return Err(status(StatusCode::BAD_REQUEST, "bad device")); }
            let r: PostReq = parse(body)?;
            if !is_hex(&r.id, 32) { return Err(status(StatusCode::BAD_REQUEST, "bad id")); }
            let data = dec(&r.data, MAX_WELCOME_BYTES)?;
            let revoked: Option<Option<String>> = db.query_row("SELECT revoked_sig FROM devices WHERE device=?1", [dev], |r| r.get(0)).optional().map_err(dberr)?;
            if !matches!(revoked, Some(None)) { return Err(status(StatusCode::NOT_FOUND, "no such device")); }
            st.limited(format!("welcome|{}", a.account), lim.welcomes_per_window, "too many invitations sent")?;
            let (n, bytes): (u64, u64) = db.query_row("SELECT COUNT(*), COALESCE(SUM(LENGTH(data)),0) FROM welcomes WHERE device=?1", [dev], |r| Ok((r.get(0)?, r.get(1)?))).map_err(dberr)?;
            if n >= lim.max_inbox_items || bytes + data.len() as u64 > lim.max_inbox_bytes { return Err(status(StatusCode::TOO_MANY_REQUESTS, "inbox full")); }
            db.execute("INSERT OR IGNORE INTO welcomes(device,id,data) VALUES(?1,?2,?3)", params![dev, r.id, data]).map_err(dberr)?;
            st.tick.send_modify(|t| *t += 1);
            Ok(status(StatusCode::OK, "ok"))
        }
        // ---- pairing rendezvous: unguessable id, short lived. Reading needs no account (the new device has
        // none yet). Writing does: the approving device is registered, and the new device writes its answer
        // after it has registered. So this mailbox is covered by the registration gate and per-account limits.
        (Method::PUT, ["v1", "rendezvous", rid, slot]) => {
            let a = authenticate(&db, headers, ms, path, body, true)?.1.unwrap();
            if !is_hex(rid, 32) || !matches!(*slot, "a" | "b") || body.len() > MAX_RENDEZVOUS_BYTES { return Err(status(StatusCode::BAD_REQUEST, "bad")); }
            let now = now_secs() as i64;
            db.execute("DELETE FROM rendezvous WHERE created < ?1", [now - 900]).map_err(dberr)?;
            let n: u64 = db.query_row("SELECT COUNT(*) FROM rendezvous", [], |r| r.get(0)).map_err(dberr)?;
            if n >= lim.max_rendezvous_total { return Err(status(StatusCode::TOO_MANY_REQUESTS, "busy")); }
            let mine: u64 = db.query_row("SELECT COUNT(*) FROM rendezvous WHERE owner=?1", [&a.account], |r| r.get(0)).map_err(dberr)?;
            if mine >= lim.max_rendezvous_per_account { return Err(status(StatusCode::TOO_MANY_REQUESTS, "too many pairings at once")); }
            // First writer keeps the slot: nobody can swap what is waiting there. The same account may repeat itself.
            let held: Option<Option<String>> = db.query_row("SELECT owner FROM rendezvous WHERE rid=?1 AND slot=?2", params![rid, slot], |r| r.get(0)).optional().map_err(dberr)?;
            if held.is_some_and(|o| o.as_deref() != Some(a.account.as_str())) { return Err(status(StatusCode::CONFLICT, "slot taken")); }
            db.execute("INSERT OR REPLACE INTO rendezvous(rid,slot,data,created,owner) VALUES(?1,?2,?3,?4,?5)", params![rid, slot, body, now, a.account]).map_err(dberr)?;
            Ok(status(StatusCode::OK, "ok"))
        }
        (Method::GET, ["v1", "rendezvous", rid, slot]) => {
            let now = now_secs() as i64;
            let d: Option<Vec<u8>> = db.query_row("SELECT data FROM rendezvous WHERE rid=?1 AND slot=?2 AND created>=?3", params![rid, slot, now - 900], |r| r.get(0)).optional().map_err(dberr)?;
            match d {
                Some(b) => Ok(Response::builder().status(200).body(Body::from(b)).unwrap()),
                None => Err(status(StatusCode::NOT_FOUND, "none")),
            }
        }
        _ => Err(status(StatusCode::NOT_FOUND, "not found")),
    }
}

fn group_exists(db: &Connection, gid: &str) -> bool {
    db.query_row("SELECT COUNT(*) FROM group_members WHERE gid=?1", [gid], |r| r.get::<_, i64>(0)).unwrap_or(0) > 0
}

/// One sync response, kept under a byte budget. Without one, a member could post enough into a group to make
/// every other member's response too big to read, and nobody could ever catch up.
fn collect(st: &RelayState, a: &Authed, req: &SyncReq) -> rusqlite::Result<SyncResp> {
    let db = st.db.lock().unwrap();
    let mut out = SyncResp::default();
    let budget = req.max_bytes.unwrap_or(SYNC_MAX_BYTES).clamp(SYNC_MIN_BYTES, SYNC_MAX_BYTES);
    let mut used = 0u64;
    let mut any = false; // one entry always goes out, whatever its size: that is what guarantees progress
    for w in req.ack_welcomes.iter().take(1000) { db.execute("DELETE FROM welcomes WHERE device=?1 AND id=?2", params![a.device, w])?; }
    {
        let mut s = db.prepare("SELECT id,data FROM welcomes WHERE device=?1 ORDER BY rowid ASC LIMIT 50")?;
        let mut rows = s.query([&a.device])?;
        while let Some(r) = rows.next()? {
            let data = b64(&r.get::<_, Vec<u8>>(1)?);
            let size = data.len() as u64 + 80;
            if any && used + size > budget / 2 { break; } // the rest stay in the inbox for the next response
            used += size; any = true;
            out.welcomes.push(WelcomeEntry { id: r.get(0)?, data });
        }
    }
    // Start at a random group, and let none take more than a share, so a noisy one cannot starve the others.
    let groups: Vec<(&String, &u64)> = req.cursors.iter().take(2000).collect();
    let start = if groups.is_empty() { 0 } else { rand::thread_rng().gen_range(0..groups.len()) };
    let share = (budget / 4).max(SYNC_MIN_BYTES);
    for (gid, after) in groups[start..].iter().chain(groups[..start].iter()).copied() {
        if !valid_gid(gid) { continue; }
        let ok: i64 = db.query_row("SELECT COUNT(*) FROM group_members WHERE gid=?1 AND account=?2", params![gid, a.account], |r| r.get(0))?;
        if ok == 0 { out.denied.push(gid.clone()); continue; }
        let mut s = db.prepare_cached("SELECT seq,id,data FROM entries WHERE gid=?1 AND seq>?2 ORDER BY seq ASC LIMIT 300")?;
        let mut rows = s.query(params![gid, after])?;
        let (mut v, mut taken, mut more) = (Vec::new(), 0u64, false);
        while let Some(r) = rows.next()? {
            let size = r.get_ref(2)?.as_blob().map(|b| b.len()).unwrap_or(0) as u64 * 4 / 3 + 120;
            if any && (used + size > budget || taken + size > share) { more = true; break; }
            let e = Entry { seq: r.get(0)?, id: r.get(1)?, data: b64(&r.get::<_, Vec<u8>>(2)?) };
            used += size; taken += size; any = true;
            v.push(e);
        }
        if v.len() == 300 { more = true; }
        if more { out.more.push(gid.clone()); }
        if !v.is_empty() { out.messages.insert(gid.clone(), v); }
    }
    Ok(out)
}

async fn sync(st: Arc<RelayState>, headers: &HeaderMap, path: &str, body: &[u8]) -> Resp {
    let a = {
        let db = st.db.lock().unwrap();
        match authenticate(&db, headers, "POST", path, body, true) { Ok((_, Some(a))) => a, Ok(_) => return status(StatusCode::FORBIDDEN, "x"), Err(r) => return r }
    };
    let req: SyncReq = match parse(body) { Ok(r) => r, Err(r) => return r };
    let deadline = tokio::time::Instant::now() + Duration::from_millis(req.wait_ms.min(30_000));
    let mut rx = st.tick.subscribe();
    let mut first = true;
    loop {
        rx.borrow_and_update();
        // acks are processed on the first pass only, so a long-poll doesn't re-delete
        let mut r2 = req.clone();
        if !first { r2.ack_welcomes.clear(); }
        first = false;
        let st2 = st.clone();
        let a2 = Authed { device: a.device.clone(), account: a.account.clone() };
        let res = tokio::task::spawn_blocking(move || collect(&st2, &a2, &r2)).await;
        let out = match res { Ok(Ok(o)) => o, _ => return status(StatusCode::INTERNAL_SERVER_ERROR, "db") };
        if !out.messages.is_empty() || !out.welcomes.is_empty() || tokio::time::Instant::now() >= deadline || req.wait_ms == 0 {
            return json(&out);
        }
        let _ = tokio::time::timeout_at(deadline, rx.changed()).await;
    }
}
