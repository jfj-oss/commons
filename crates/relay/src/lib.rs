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
use rusqlite::{params, Connection, OptionalExtension};
use std::{net::SocketAddr, path::PathBuf, sync::{Arc, Mutex}, time::Duration};
use tokio::sync::watch;

pub struct RelayState {
    db: Mutex<Connection>,
    blob_dir: PathBuf,
    tick: watch::Sender<u64>,
    livekit: Option<LiveKitCfg>,
}

/// Optional call support: the relay signs short-lived LiveKit join tokens for current members of a group.
/// It holds the LiveKit API secret, so it can mint tokens but sees no media and no keys.
#[derive(Clone)]
pub struct LiveKitCfg {
    pub api_key: String,
    pub api_secret: String,
    /// The websocket URL clients should connect to (wss://... in production).
    pub url: String,
    pub token_ttl_secs: u64,
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
         CREATE TABLE IF NOT EXISTS entries(gid TEXT NOT NULL, seq INTEGER NOT NULL, id TEXT NOT NULL, data BLOB NOT NULL, PRIMARY KEY(gid,seq), UNIQUE(gid,id));
         CREATE TABLE IF NOT EXISTS welcomes(device TEXT NOT NULL, id TEXT NOT NULL, data BLOB NOT NULL, PRIMARY KEY(device,id));
         CREATE TABLE IF NOT EXISTS call_denied(gid TEXT NOT NULL, account TEXT NOT NULL, PRIMARY KEY(gid,account));
         CREATE TABLE IF NOT EXISTS rendezvous(rid TEXT NOT NULL, slot TEXT NOT NULL, data BLOB NOT NULL, created INTEGER NOT NULL, PRIMARY KEY(rid,slot));",
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
pub async fn start(bind: SocketAddr, data_dir: PathBuf) -> std::io::Result<RelayHandle> { start_with(bind, data_dir, None).await }

pub async fn start_with(bind: SocketAddr, data_dir: PathBuf, livekit: Option<LiveKitCfg>) -> std::io::Result<RelayHandle> {
    std::fs::create_dir_all(data_dir.join("blobs"))?;
    let db = open_db(data_dir.join("relay.db").to_str().unwrap()).map_err(|e| std::io::Error::other(e.to_string()))?;
    let (tick, _) = watch::channel(0u64);
    let st = Arc::new(RelayState { db: Mutex::new(db), blob_dir: data_dir.join("blobs"), tick, livekit });
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
    let dev = verify_request(h, method, path, body).map_err(|_| status(StatusCode::UNAUTHORIZED, "bad auth"))?;
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

async fn handle(State(st): State<Arc<RelayState>>, req: Request<Body>) -> Resp {
    let (parts, body) = req.into_parts();
    let limit = MAX_BLOB_BYTES + 1024;
    let body = match to_bytes(body, limit).await { Ok(b) => b, Err(_) => return status(StatusCode::PAYLOAD_TOO_LARGE, "too large") };
    let path = parts.uri.path().to_string();
    let query = parts.uri.query().unwrap_or("").to_string();
    let m = parts.method.clone();
    let _ = query;
    // The long-poll endpoint must not hold the DB lock while waiting.
    if m == Method::POST && path == "/v1/sync" {
        return sync(st, &parts.headers, &path, &body).await;
    }
    let st2 = st.clone();
    let headers = parts.headers.clone();
    // Everything else is short: run it on a blocking thread so the sqlite mutex never blocks the runtime.
    tokio::task::spawn_blocking(move || {
        let seg: Vec<&str> = path.trim_matches('/').split('/').collect();
        route(&st2, &m, m.as_str(), &seg, &headers, &path, &body).unwrap_or_else(|r| r)
    }).await.unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR, "panic"))
}

fn route(st: &RelayState, m: &Method, ms: &str, seg: &[&str], headers: &HeaderMap, path: &str, body: &[u8]) -> Result<Resp, Resp> {
    let db = st.db.lock().unwrap();
    match (m.clone(), seg) {
        (Method::GET, ["health"]) => Ok(status(StatusCode::OK, "ok")),
        // ---- device registry ----
        (Method::POST, ["v1", "devices"]) => {
            let (dev, _) = authenticate(&db, headers, ms, path, body, false)?;
            let r: RegisterReq = parse(body)?;
            if r.cert.device != dev || r.cert.verify().is_err() { return Err(status(StatusCode::BAD_REQUEST, "bad cert")); }
            // A revoked device id can never come back (checked in authenticate for registered ones).
            let n = db.execute("INSERT OR IGNORE INTO devices(device,account,cert) VALUES(?1,?2,?3)",
                params![r.cert.device, r.cert.account, serde_json::to_string(&r.cert).unwrap()]).map_err(dberr)?;
            let _ = n;
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
            for p in &r.packages {
                let d = dec(p, 8192)?;
                db.execute("INSERT INTO keypackages(device,data,last_resort) VALUES(?1,?2,0)", params![a.device, d]).map_err(dberr)?;
            }
            if let Some(lr) = &r.last_resort {
                let d = dec(lr, 8192)?;
                db.execute("DELETE FROM keypackages WHERE device=?1 AND last_resort=1", [&a.device]).map_err(dberr)?;
                db.execute("INSERT INTO keypackages(device,data,last_resort) VALUES(?1,?2,1)", params![a.device, d]).map_err(dberr)?;
            }
            Ok(status(StatusCode::OK, "ok"))
        }
        (Method::GET, ["v1", "keypackages", dev]) => {
            authenticate(&db, headers, ms, path, body, true)?;
            if !is_hex(dev, 64) { return Err(status(StatusCode::BAD_REQUEST, "bad device")); }
            let revoked: Option<Option<String>> = db.query_row("SELECT revoked_sig FROM devices WHERE device=?1", [dev], |r| r.get(0)).optional().map_err(dberr)?;
            if !matches!(revoked, Some(None)) { return Ok(json(&KpResp { package: None })); }
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
            let n: i64 = db.query_row("SELECT COUNT(*) FROM group_members WHERE gid=?1", [gid], |r| r.get(0)).map_err(dberr)?;
            if n == 0 {
                db.execute("INSERT INTO group_members(gid,account) VALUES(?1,?2)", params![gid, a.account]).map_err(dberr)?;
            } else {
                let ok: i64 = db.query_row("SELECT COUNT(*) FROM group_members WHERE gid=?1 AND account=?2", params![gid, a.account], |r| r.get(0)).map_err(dberr)?;
                if ok == 0 { return Err(status(StatusCode::FORBIDDEN, "not a member")); }
            }
            for x in r.add.iter().filter(|x| is_hex(x, 64)) {
                db.execute("INSERT OR IGNORE INTO group_members(gid,account) VALUES(?1,?2)", params![gid, x]).map_err(dberr)?;
                db.execute("DELETE FROM call_denied WHERE gid=?1 AND account=?2", params![gid, x]).map_err(dberr)?;
            }
            for x in r.call_remove.iter().filter(|x| is_hex(x, 64)) {
                db.execute("INSERT OR IGNORE INTO call_denied(gid,account) VALUES(?1,?2)", params![gid, x]).map_err(dberr)?;
            }
            for x in r.remove.iter() { db.execute("DELETE FROM group_members WHERE gid=?1 AND account=?2", params![gid, x]).map_err(dberr)?; }
            Ok(status(StatusCode::OK, "ok"))
        }
        // ---- calls: a short-lived LiveKit join token for a current member. The media server is untrusted and
        // never sees the group id (the room name is a one-way hash of it); the key comes from MLS, not from here.
        (Method::POST, ["v1", "groups", gid, "call-token"]) => {
            let a = authenticate(&db, headers, ms, path, body, true)?.1.unwrap();
            let Some(cfg) = st.livekit.as_ref() else { return Err(status(StatusCode::NOT_FOUND, "this relay has no call server configured")) };
            if !valid_gid(gid) { return Err(status(StatusCode::BAD_REQUEST, "bad group")); }
            let member: i64 = db.query_row("SELECT COUNT(*) FROM group_members WHERE gid=?1 AND account=?2", params![gid, a.account], |r| r.get(0)).map_err(dberr)?;
            let denied: i64 = db.query_row("SELECT COUNT(*) FROM call_denied WHERE gid=?1 AND account=?2", params![gid, a.account], |r| r.get(0)).map_err(dberr)?;
            if member == 0 || denied > 0 { return Err(status(StatusCode::FORBIDDEN, "not a current member")); }
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
            let member: i64 = db.query_row("SELECT COUNT(*) FROM group_members WHERE gid=?1 AND account=?2", params![gid, a.account], |r| r.get(0)).map_err(dberr)?;
            // Join requests come from devices that are not yet members.
            let is_join = data[0] == TAG_JOIN_REQUEST;
            if member == 0 && !(is_join && group_exists(&db, gid)) { return Err(status(StatusCode::FORBIDDEN, "not a member")); }
            if is_join && data.len() > 4096 { return Err(status(StatusCode::PAYLOAD_TOO_LARGE, "too large")); }
            if let Some(seq) = db.query_row("SELECT seq FROM entries WHERE gid=?1 AND id=?2", params![gid, r.id], |r| r.get::<_, u64>(0)).optional().map_err(dberr)? {
                return Ok(json(&PostResp { seq, duplicate: true }));
            }
            let seq: u64 = db.query_row("SELECT COALESCE(MAX(seq),0)+1 FROM entries WHERE gid=?1", [gid], |r| r.get(0)).map_err(dberr)?;
            db.execute("INSERT INTO entries(gid,seq,id,data) VALUES(?1,?2,?3,?4)", params![gid, seq, r.id, data]).map_err(dberr)?;
            st.tick.send_modify(|t| *t += 1);
            Ok(json(&PostResp { seq, duplicate: false }))
        }
        // ---- welcome inbox (per device) ----
        (Method::POST, ["v1", "welcomes", dev]) => {
            authenticate(&db, headers, ms, path, body, true)?;
            if !is_hex(dev, 64) { return Err(status(StatusCode::BAD_REQUEST, "bad device")); }
            let r: PostReq = parse(body)?;
            if !is_hex(&r.id, 32) { return Err(status(StatusCode::BAD_REQUEST, "bad id")); }
            let data = dec(&r.data, MAX_WELCOME_BYTES)?;
            let n: i64 = db.query_row("SELECT COUNT(*) FROM welcomes WHERE device=?1", [dev], |r| r.get(0)).map_err(dberr)?;
            if n > 2000 { return Err(status(StatusCode::TOO_MANY_REQUESTS, "inbox full")); }
            db.execute("INSERT OR IGNORE INTO welcomes(device,id,data) VALUES(?1,?2,?3)", params![dev, r.id, data]).map_err(dberr)?;
            st.tick.send_modify(|t| *t += 1);
            Ok(status(StatusCode::OK, "ok"))
        }
        // ---- encrypted blobs ----
        (Method::PUT, ["v1", "blobs", id]) => {
            authenticate(&db, headers, ms, path, body, true)?;
            if !is_hex(id, 32) { return Err(status(StatusCode::BAD_REQUEST, "bad id")); }
            if body.len() > MAX_BLOB_BYTES { return Err(status(StatusCode::PAYLOAD_TOO_LARGE, "too large")); }
            let p = st.blob_dir.join(id);
            if p.exists() { return Err(status(StatusCode::CONFLICT, "exists")); }
            let tmp = st.blob_dir.join(format!("{id}.tmp"));
            std::fs::write(&tmp, body).and_then(|_| std::fs::rename(&tmp, &p)).map_err(dberr)?;
            Ok(status(StatusCode::OK, "ok"))
        }
        (Method::GET, ["v1", "blobs", id]) => {
            authenticate(&db, headers, ms, path, body, true)?;
            if !is_hex(id, 32) { return Err(status(StatusCode::BAD_REQUEST, "bad id")); }
            match std::fs::read(st.blob_dir.join(id)) {
                Ok(b) => Ok(Response::builder().status(200).header("content-type", "application/octet-stream").body(Body::from(b)).unwrap()),
                Err(_) => Err(status(StatusCode::NOT_FOUND, "no such blob")),
            }
        }
        // ---- pairing rendezvous: unauthenticated, unguessable id, short lived ----
        (Method::PUT, ["v1", "rendezvous", rid, slot]) => {
            if !is_hex(rid, 32) || !matches!(*slot, "a" | "b") || body.len() > MAX_RENDEZVOUS_BYTES { return Err(status(StatusCode::BAD_REQUEST, "bad")); }
            let now = now_secs() as i64;
            db.execute("DELETE FROM rendezvous WHERE created < ?1", [now - 900]).map_err(dberr)?;
            let n: i64 = db.query_row("SELECT COUNT(*) FROM rendezvous", [], |r| r.get(0)).map_err(dberr)?;
            if n > 5000 { return Err(status(StatusCode::TOO_MANY_REQUESTS, "busy")); }
            db.execute("INSERT OR REPLACE INTO rendezvous(rid,slot,data,created) VALUES(?1,?2,?3,?4)", params![rid, slot, body, now]).map_err(dberr)?;
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

fn collect(st: &RelayState, a: &Authed, req: &SyncReq) -> rusqlite::Result<SyncResp> {
    let db = st.db.lock().unwrap();
    let mut out = SyncResp::default();
    for (gid, after) in req.cursors.iter().take(2000) {
        if !valid_gid(gid) { continue; }
        let ok: i64 = db.query_row("SELECT COUNT(*) FROM group_members WHERE gid=?1 AND account=?2", params![gid, a.account], |r| r.get(0))?;
        if ok == 0 { out.denied.push(gid.clone()); continue; }
        let mut s = db.prepare("SELECT seq,id,data FROM entries WHERE gid=?1 AND seq>?2 ORDER BY seq ASC LIMIT 300")?;
        let rows = s.query_map(params![gid, after], |r| Ok(Entry { seq: r.get(0)?, id: r.get(1)?, data: b64(&r.get::<_, Vec<u8>>(2)?) }))?;
        let v: Vec<Entry> = rows.collect::<Result<_, _>>()?;
        if !v.is_empty() { out.messages.insert(gid.clone(), v); }
    }
    for w in &req.ack_welcomes { db.execute("DELETE FROM welcomes WHERE device=?1 AND id=?2", params![a.device, w])?; }
    let mut s = db.prepare("SELECT id,data FROM welcomes WHERE device=?1 ORDER BY rowid ASC LIMIT 50")?;
    let rows = s.query_map([&a.device], |r| Ok(WelcomeEntry { id: r.get(0)?, data: b64(&r.get::<_, Vec<u8>>(1)?) }))?;
    out.welcomes = rows.collect::<Result<_, _>>()?;
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
