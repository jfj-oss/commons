//! Commons relay: an append-only, per-group log of opaque bytes plus a key-package directory,
//! a welcome inbox, an encrypted blob store and a tiny rendezvous mailbox for device pairing.
//! It assumes it is hostile: it holds no key that decrypts anything. See docs/THREAT-MODEL.md
//! for the exact list of what it can observe.
pub mod front;
pub mod live;
use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{HeaderMap, Method, Request, Response, StatusCode},
    Router,
};
use commons_proto::*;
use rand::Rng;
use rusqlite::{params, Connection, OptionalExtension};
use std::{collections::{BTreeSet, HashMap}, net::SocketAddr, path::PathBuf, sync::{Arc, Mutex}, time::Duration};
use tokio::sync::{watch, Semaphore};

/// Who may use this relay and how much of it. The defaults suit a few friends on one small server; every limit
/// is per account or per device, so one person cannot use up what the others need.
#[derive(Clone, Debug)]
pub struct Limits {
    /// Optional call support: when set, current members can fetch LiveKit join tokens (see `LiveKitCfg`).
    pub livekit: Option<LiveKitCfg>,
    /// When set, `/join` shows friends how to get in, and the files in this folder can be downloaded from `/download/`.
    pub site_dir: Option<PathBuf>,
    /// Apps older than this are told, each time they start, that they must update (`minimum_version` in the update
    /// manifest; docs/UPDATES.md). Advisory: nothing is refused because of it.
    pub min_app_version: Option<String>,
    /// Call tokens one device may fetch per window (reconnects need a few, loops should not).
    pub call_tokens_per_window: u32,
    /// Presence checks one device may make per window. One check asks about all the rooms on screen.
    pub call_presence_per_window: u32,
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
    /// Unauthenticated device revocations, whole relay.
    pub revokes_per_window: u32,
    /// How long a request body may take to arrive (ordinary requests, and blob uploads).
    pub body_timeout: Duration,
    pub big_body_timeout: Duration,
    /// Invite-secret checks (POST /v1/info), whole relay. Kept apart from registrations so probing cannot lock out sign-ups.
    pub info_checks_per_window: u32,
    /// How much large-file storage one member may hold at once.
    pub large_file_quota_per_account: u64,
    /// The same for the owner of the community a file belongs to (default 100 GB).
    pub large_file_quota_owner: u64,
    /// Most large-file bytes the whole relay holds at once, across every account.
    pub large_file_total: u64,
    /// How long a large file stays before the relay removes it.
    pub large_file_expire_secs: u64,
    /// Largest large file a client may announce (a sanity bound, not a quota).
    pub large_file_max_bytes: u64,
    /// Begins (fresh or resumed) one device may ask for per window.
    pub large_begins_per_window: u32,
    /// The live channel's in-memory buffer (see `live.rs`). Nothing in it is stored.
    pub live: live::LiveLimits,
}
impl Default for Limits {
    fn default() -> Self {
        Limits {
            invite_secret: None,
            livekit: None,
            site_dir: None,
            min_app_version: None,
            call_tokens_per_window: 60,
            call_presence_per_window: 1200,
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
            revokes_per_window: 120,
            body_timeout: Duration::from_secs(15),
            big_body_timeout: Duration::from_secs(120),
            info_checks_per_window: 120,
            large_file_quota_per_account: 10 << 30,
            large_file_quota_owner: 100 << 30,
            large_file_total: 250 << 30,
            large_file_expire_secs: 7 * 86400,
            large_file_max_bytes: 1 << 40,
            large_begins_per_window: 600,
            live: live::LiveLimits::default(),
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
    /// Where this relay reaches LiveKit's server API, when that is not `url`. Behind `--front` only `/rtc` is public on
    /// purpose, so the API has to be asked on LiveKit's own local port.
    pub api_url: Option<String>,
    /// LiveKit always tells clients TURN over TLS is on port 443. When it is somewhere else, clients are told here.
    pub turn_port: Option<u16>,
}
impl std::fmt::Debug for LiveKitCfg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "LiveKitCfg {{ url: {:?}, api_key: {:?}, api_secret: <hidden> }}", self.url, self.api_key) }
}

#[derive(serde::Serialize)]
struct LkVideoGrant { room: String, #[serde(rename = "roomJoin")] room_join: bool, #[serde(rename = "canPublish")] can_publish: bool, #[serde(rename = "canSubscribe")] can_subscribe: bool, #[serde(rename = "canPublishData")] can_publish_data: bool,
    /// Lets a client set small attributes on itself (the call's "deafened" flag); no message content travels this way.
    #[serde(rename = "canUpdateOwnMetadata")] can_update_own_metadata: bool }
#[derive(serde::Serialize)]
struct LkClaims { iss: String, sub: String, jti: String, nbf: u64, exp: u64, video: LkVideoGrant }

/// Mint a LiveKit access token (HS256 JWT, standard `jsonwebtoken` crate). Public so tests can verify the exact claims.
pub fn mint_livekit_token(cfg: &LiveKitCfg, identity: &str, room: &str, now: u64) -> Result<(String, u64), jsonwebtoken::errors::Error> {
    let exp = now + cfg.token_ttl_secs;
    let claims = LkClaims { iss: cfg.api_key.clone(), sub: identity.into(), jti: identity.into(), nbf: now.saturating_sub(30), exp,
        video: LkVideoGrant { room: room.into(), room_join: true, can_publish: true, can_subscribe: true, can_publish_data: false, can_update_own_metadata: true } };
    let tok = jsonwebtoken::encode(&jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256), &claims, &jsonwebtoken::EncodingKey::from_secret(cfg.api_secret.as_bytes()))?;
    Ok((tok, exp))
}

#[derive(serde::Serialize)]
struct LkAdminGrant { room: String, #[serde(rename = "roomAdmin")] room_admin: bool }
#[derive(serde::Serialize)]
struct LkAdminClaims { iss: String, sub: String, nbf: u64, exp: u64, video: LkAdminGrant }

/// A short-lived token for LiveKit's server API (`RoomService.ListParticipants`). It never leaves the relay.
fn mint_livekit_admin_token(cfg: &LiveKitCfg, room: &str, now: u64) -> Result<String, jsonwebtoken::errors::Error> {
    let claims = LkAdminClaims { iss: cfg.api_key.clone(), sub: cfg.api_key.clone(), nbf: now.saturating_sub(10), exp: now + 60,
        video: LkAdminGrant { room: room.into(), room_admin: true } };
    jsonwebtoken::encode(&jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256), &claims, &jsonwebtoken::EncodingKey::from_secret(cfg.api_secret.as_bytes()))
}

/// LiveKit's media URL is a websocket URL; the server API is the same host over HTTP.
fn livekit_http_url(url: &str) -> Option<String> {
    let u = url.trim_end_matches('/');
    let base = if let Some(r) = u.strip_prefix("wss://") { format!("https://{r}") }
        else if let Some(r) = u.strip_prefix("ws://") { format!("http://{r}") }
        else if u.starts_with("https://") || u.starts_with("http://") { u.to_string() }
        else { return None };
    Some(format!("{base}/twirp/livekit.RoomService/ListParticipants"))
}

#[derive(serde::Deserialize)]
struct LkParticipants { #[serde(default)] participants: Vec<LkParticipant> }
#[derive(serde::Deserialize)]
struct LkTrack {
    /// Protojson writes the TrackType enum as `"type"`; the source says which audio track is the microphone.
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    muted: bool,
}
#[derive(serde::Deserialize)]
struct LkParticipant {
    identity: String,
    #[serde(default)]
    state: serde_json::Value,
    #[serde(default)]
    tracks: Vec<LkTrack>,
    #[serde(default)]
    attributes: HashMap<String, String>,
}

/// What one LiveKit participant reports about itself. Microphone muted comes from the media server's own track
/// state; the `deaf` and `speaking` flags are small participant attributes the call client sets (see call.js).
/// No microphone published counts as muted, which is what a listener sees and matches the call panel.
fn lk_state(p: &LkParticipant) -> CallPeerState {
    let mic = p.tracks.iter().find(|t| {
        let source = t.source.to_ascii_uppercase();
        source == "MICROPHONE" || (t.kind.eq_ignore_ascii_case("AUDIO") && source != "SCREEN_SHARE_AUDIO")
    });
    CallPeerState {
        muted: mic.map_or(true, |t| t.muted),
        deafened: p.attributes.get("deaf").is_some_and(|v| v == "1"),
        speaking: p.attributes.get("speaking").is_some_and(|v| v == "1"),
    }
}

/// Ask LiveKit who is connected to one room, and what each device reports about its microphone and flags. A 404
/// (a room nobody has ever called in) means nobody there. A transport error is reported, so the caller can keep its
/// last answer instead of showing an empty call just because the media server was briefly unreachable.
fn livekit_participants(cfg: &LiveKitCfg, gid: &str) -> Result<Vec<(String, CallPeerState)>, String> {
    let url = livekit_http_url(cfg.api_url.as_deref().unwrap_or(&cfg.url)).ok_or_else(|| "the call server's address is not a websocket or http URL".to_string())?;
    let room = call_room(gid);
    let token = mint_livekit_admin_token(cfg, &room, now_secs()).map_err(|_| "cannot sign a call server request".to_string())?;
    let body = serde_json::json!({ "room": room }).to_string();
    let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(2)).build();
    let answer = agent.post(&url).timeout(Duration::from_secs(4))
        .set("authorization", &format!("Bearer {token}")).set("content-type", "application/json").send_string(&body);
    match answer {
        Ok(r) => {
            let text = r.into_string().map_err(|e| format!("call server: {e}"))?;
            let p: LkParticipants = serde_json::from_str(&text).map_err(|_| "call server sent an answer this relay cannot read".to_string())?;
            // Identities here are device ids: this relay minted every join token, and nothing else can join its rooms.
            Ok(p.participants.into_iter()
                .filter(|x| is_hex(&x.identity, 64) && x.state.as_str() != Some("DISCONNECTED"))
                .map(|x| { let s = lk_state(&x); (x.identity, s) }).collect())
        }
        Err(ureq::Error::Status(404, _)) => Ok(vec![]),
        Err(ureq::Error::Status(c, _)) => Err(format!("call server said {c}")),
        Err(e) => Err(format!("call server: {e}")),
    }
}

pub struct RelayState {
    db: Mutex<Connection>,
    blob_dir: PathBuf,
    /// One directory per large file, and one file per chunk inside it.
    large_dir: PathBuf,
    /// Told when the server is stopping, so the expiry sweeper ends with it.
    stop: Arc<std::sync::atomic::AtomicBool>,
    tick: watch::Sender<u64>,
    /// The live channel: sealed events held in memory for a few seconds (never in the database), and a signal for pollers.
    live: Mutex<live::LiveHub>,
    live_tick: watch::Sender<u64>,
    started: std::time::Instant,
    limits: Limits,
    /// Fixed-window counters. Lost on restart, which only makes a limit briefly more generous.
    rates: Mutex<HashMap<String, (u64, u32)>>,
    /// A 25 MB body is held in memory while it is checked: only a few at a time.
    big_bodies: Arc<Semaphore>,
    /// Who LiveKit last said is in each room's call, kept for a couple of seconds so that all members' rail
    /// refreshes share one query to the media server instead of each asking it.
    presence: Mutex<HashMap<String, (u64, Vec<(String, CallPeerState)>)>>,
}

type Resp = Response<Body>;
fn status(code: StatusCode, msg: &str) -> Resp {
    Response::builder().status(code).header("content-type", "text/plain").body(Body::from(msg.to_string())).unwrap()
}
fn json<T: serde::Serialize>(v: &T) -> Resp {
    Response::builder().status(200).header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(v).unwrap())).unwrap()
}
// Lower-case only: one key has exactly one spelling, so ids cannot be aliased by changing case.
fn is_hex_byte(c: u8) -> bool { c.is_ascii_digit() || (b'a'..=b'f').contains(&c) }
fn is_hex(s: &str, n: usize) -> bool { s.len() == n && s.bytes().all(is_hex_byte) }
fn valid_gid(s: &str) -> bool { s.len() >= 8 && s.len() <= 128 && s.bytes().all(is_hex_byte) }

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
         CREATE TABLE IF NOT EXISTS large_files(id TEXT PRIMARY KEY, account TEXT NOT NULL, gid TEXT NOT NULL, size INTEGER NOT NULL,
             chunk_size INTEGER NOT NULL, chunks INTEGER NOT NULL, complete INTEGER NOT NULL DEFAULT 0, expires INTEGER NOT NULL,
             created INTEGER NOT NULL, sha256 TEXT);
         CREATE INDEX IF NOT EXISTS large_acc ON large_files(account);
         CREATE INDEX IF NOT EXISTS large_exp ON large_files(expires);
         CREATE TABLE IF NOT EXISTS usage(account TEXT PRIMARY KEY, entry_bytes INTEGER NOT NULL DEFAULT 0);",
    )?;
    Ok(c)
}

pub struct RelayHandle {
    pub addr: SocketAddr,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<()>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}
impl RelayHandle {
    pub fn url(&self) -> String { format!("http://{}", self.addr) }
    pub async fn stop(mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
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
    std::fs::create_dir_all(data_dir.join("large"))?;
    let db = open_db(data_dir.join("relay.db").to_str().unwrap()).map_err(|e| std::io::Error::other(e.to_string()))?;
    let (tick, _) = watch::channel(0u64);
    let (live_tick, _) = watch::channel(0u64);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let st = Arc::new(RelayState { db: Mutex::new(db), blob_dir: data_dir.join("blobs"), large_dir: data_dir.join("large"), stop: stop.clone(), tick, live: Mutex::new(live::LiveHub::new(hex::encode(rand::thread_rng().gen::<[u8; 8]>()), limits.live.clone())), live_tick, started: std::time::Instant::now(), limits, rates: Mutex::new(HashMap::new()), big_bodies: Arc::new(Semaphore::new(4)), presence: Mutex::new(HashMap::new()) });
    // A chunk directory with no row is what a crash mid-upload leaves behind; the row is written first, so this is safe.
    {
        let db = st.db.lock().unwrap();
        if let Ok(rd) = std::fs::read_dir(&st.large_dir) {
            for e in rd.flatten() {
                let id = e.file_name().to_string_lossy().to_string();
                let known: i64 = db.query_row("SELECT COUNT(*) FROM large_files WHERE id=?1", [&id], |r| r.get(0)).unwrap_or(0);
                if known == 0 { let _ = std::fs::remove_dir_all(e.path()); }
            }
        }
        sweep_large_locked(&db, &st);
    }
    // Files expire even if nobody touches them; a minute of lag is harmless.
    {
        let st2 = st.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                if st2.stop.load(std::sync::atomic::Ordering::Relaxed) { break; }
                let db = st2.db.lock().unwrap();
                sweep_large_locked(&db, &st2);
            }
        });
    }
    // The live buffer forgets by itself even when nobody is polling.
    {
        let st2 = st.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                if st2.stop.load(std::sync::atomic::Ordering::Relaxed) { break; }
                let now = st2.live_now();
                st2.live.lock().unwrap().sweep(now);
            }
        });
    }
    let app = Router::new().fallback(handle).with_state(st);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    let addr = listener.local_addr()?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let join = tokio::spawn(async move {
        let _ = axum::serve(listener, app).with_graceful_shutdown(async { let _ = rx.await; }).await;
    });
    Ok(RelayHandle { addr, shutdown: Some(tx), join: Some(join), stop })
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
    fn live_now(&self) -> u64 { self.started.elapsed().as_millis() as u64 }
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
    if *m == Method::PUT && path.starts_with("/v1/blobs/") { MAX_BLOB_BYTES + 1024 }
    else if *m == Method::PUT && path.starts_with("/v1/large-files/") { MAX_LARGE_CHUNK_BYTES as usize + 1024 }
    else { 4 * 1024 * 1024 }
}

/// The pages a friend sees before they have the app. Fixed files plus one flat folder: nothing here touches accounts.
const JOIN_CSP: &str = "default-src 'none'; img-src 'self'; style-src 'unsafe-inline'; script-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'";
fn page(ty: &'static str, body: &'static str) -> Resp {
    Response::builder().header("content-type", ty).header("content-security-policy", JOIN_CSP).header("x-content-type-options", "nosniff").header("referrer-policy", "no-referrer")
        .body(Body::from(body)).unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR, "page"))
}
/// A static picture for the join page (favicons), with the same headers as page().
fn page_bytes(ty: &'static str, body: &'static [u8]) -> Resp {
    Response::builder().header("content-type", ty).header("x-content-type-options", "nosniff").header("cache-control", "public, max-age=86400")
        .body(Body::from(body)).unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR, "page"))
}
/// A plain file name: no path, nothing hidden.
fn download_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 200 && !n.starts_with('.') && n.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_' | b'+'))
}
/// `major.minor.patch`, optionally `-pre`: what `--min-app-version` accepts.
pub fn is_app_version(v: &str) -> bool {
    let (core, pre) = v.split_once('-').unwrap_or((v, "x"));
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.len() <= 9 && p.bytes().all(|c| c.is_ascii_digit()))
        && !pre.is_empty() && pre.len() <= 32 && pre.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'.')
}
const MAX_MANIFEST: u64 = 256 * 1024;
/// The update manifest that scripts/publish-update.sh wrote, with this relay's minimum version laid over it.
async fn update_manifest(dir: &std::path::Path, min: Option<&str>) -> Resp {
    let p = dir.join("update").join("latest.json");
    let Ok(md) = tokio::fs::metadata(&p).await else { return status(StatusCode::NOT_FOUND, "no updates published") };
    if !md.is_file() || md.len() > MAX_MANIFEST { return status(StatusCode::NOT_FOUND, "no updates published"); }
    let Ok(b) = tokio::fs::read(&p).await else { return status(StatusCode::NOT_FOUND, "no updates published") };
    let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(&b) else { tracing::warn!("{} is not valid JSON", p.display()); return status(StatusCode::NOT_FOUND, "no updates published") };
    if let (Some(m), Some(o)) = (min, v.as_object_mut()) { o.insert("minimum_version".into(), serde_json::Value::String(m.into())); }
    Response::builder().header("content-type", "application/json").header("cache-control", "no-store").header("x-content-type-options", "nosniff")
        .body(Body::from(serde_json::to_vec(&v).unwrap_or_default())).unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR, "manifest"))
}
async fn send_file(dir: &std::path::Path, name: &str) -> Resp {
    if !download_name(name) { return status(StatusCode::NOT_FOUND, "no such file"); }
    let Ok(f) = tokio::fs::File::open(dir.join(name)).await else { return status(StatusCode::NOT_FOUND, "no such file") };
    let Ok(md) = f.metadata().await else { return status(StatusCode::NOT_FOUND, "no such file") };
    if !md.is_file() { return status(StatusCode::NOT_FOUND, "no such file"); }
    Response::builder().header("content-type", "application/octet-stream").header("content-length", md.len()).header("content-disposition", format!("attachment; filename=\"{name}\"")).header("x-content-type-options", "nosniff")
        .body(Body::from_stream(tokio_util::io::ReaderStream::new(f))).unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR, "file"))
}
/// `/join`, `/download/` (the files in the site folder, for friends) and `/update/` (its `update` subfolder, for the
/// apps' updater: docs/UPDATES.md). Files in `update/` are not listed on the download page.
async fn site(dir: &std::path::Path, path: &str, min: Option<&str>) -> Option<Resp> {
    if path == "/update/latest.json" { return Some(update_manifest(dir, min).await); }
    if let Some(name) = path.strip_prefix("/update/") { return Some(send_file(&dir.join("update"), name).await); }
    match path {
        "/join" => return Some(page("text/html; charset=utf-8", include_str!("join.html"))),
        "/join.js" => return Some(page("text/javascript; charset=utf-8", include_str!("join.js"))),
        "/favicon.svg" => return Some(page("image/svg+xml", include_str!("site/favicon.svg"))),
        "/favicon.ico" => return Some(page_bytes("image/x-icon", include_bytes!("site/favicon.ico"))),
        "/apple-touch-icon.png" => return Some(page_bytes("image/png", include_bytes!("site/apple-touch-icon.png"))),
        "/download/" => {
            let mut list = vec![];
            if let Ok(mut rd) = tokio::fs::read_dir(dir).await {
                while let Ok(Some(e)) = rd.next_entry().await {
                    let name = e.file_name().to_string_lossy().to_string();
                    let Ok(md) = tokio::fs::metadata(e.path()).await else { continue };
                    if md.is_file() && download_name(&name) { list.push(serde_json::json!({"name": name, "size": md.len()})); }
                }
            }
            list.sort_by_key(|f| f["name"].as_str().unwrap_or("").to_string());
            return Some(json(&list));
        }
        _ => {}
    }
    let name = path.strip_prefix("/download/")?;
    Some(send_file(dir, name).await)
}

const MAX_PRESENCE_GIDS: usize = 64;
const PRESENCE_TTL_SECS: u64 = 2;

/// `POST /v1/call-presence`: who the media server says is connected to these rooms' calls.
///
/// LiveKit is the source of truth: it drops a participant when its connection goes away (a crashed client falls
/// off after the server's ping timeout), so clients never have to announce themselves and nothing new is ever
/// put into a room's log. Members on older versions of Megatron therefore see exactly the same bytes as before;
/// this endpoint is only called by newer UIs. Privacy: an account only gets answers about rooms it is a current
/// member of and has not been cut off from calls in, the answer carries device ids (never names), and each
/// client maps those ids to its own contacts from the room's MLS group. The media server still sees only a hash
/// of the group id, as with call tokens.
async fn call_presence(st: Arc<RelayState>, headers: HeaderMap, path: &str, body: &[u8]) -> Result<Resp, Resp> {
    let Some(cfg) = st.limits.livekit.clone() else { return Err(status(StatusCode::NOT_FOUND, "this relay has no call server configured")) };
    let req: CallPresenceReq = parse(body)?;
    let wanted: BTreeSet<String> = req.gids.into_iter().filter(|g| valid_gid(g)).collect();
    if wanted.len() > MAX_PRESENCE_GIDS { return Err(status(StatusCode::PAYLOAD_TOO_LARGE, "too many rooms at once")); }
    let gids: Vec<String> = {
        let db = st.db.lock().unwrap();
        let a = authenticate(&db, &headers, "POST", path, body, true)?.1.unwrap();
        st.limited(format!("presence:{}", a.device), st.limits.call_presence_per_window, "too many presence checks, wait a little")?;
        let mut out = vec![];
        for gid in wanted {
            if !is_member(&db, &gid, &a.account)? { continue; }
            let denied: i64 = db.query_row("SELECT COUNT(*) FROM call_denied WHERE gid=?1 AND account=?2", params![&gid, &a.account], |r| r.get(0)).map_err(dberr)?;
            if denied > 0 { continue; }
            out.push(gid);
        }
        out
    };
    let now = now_secs();
    let mut rooms = std::collections::BTreeMap::new();
    let mut state = std::collections::BTreeMap::new();
    let mut ask = vec![];
    {
        let cache = st.presence.lock().unwrap();
        for gid in gids {
            match cache.get(&gid) {
                Some((at, v)) if now.saturating_sub(*at) < PRESENCE_TTL_SECS => add_presence(&mut rooms, &mut state, &gid, v),
                _ => ask.push(gid),
            }
        }
    }
    let mut handles = vec![];
    for gid in ask {
        let cfg = cfg.clone();
        let arg = gid.clone();
        handles.push((gid, tokio::task::spawn_blocking(move || livekit_participants(&cfg, &arg))));
    }
    let mut failed = false;
    let mut last_err = String::new();
    for (gid, h) in handles {
        match h.await {
            Ok(Ok(devs)) => {
                add_presence(&mut rooms, &mut state, &gid, &devs);
                let mut c = st.presence.lock().unwrap();
                if c.len() > 4096 { c.retain(|_, (at, _)| now.saturating_sub(*at) < 60); }
                c.insert(gid, (now, devs));
            }
            Ok(Err(e)) => { failed = true; last_err = e; }
            Err(_) => { failed = true; last_err = "the call server task failed".into(); }
        }
    }
    // If any room could not be asked, say so for the whole request: a client then keeps the list it already had
    // instead of briefly emptying the call of a room the media server was too busy to describe.
    if failed { return Err(status(StatusCode::BAD_GATEWAY, &format!("the call server did not answer: {last_err}"))); }
    Ok(json(&CallPresenceResp { rooms, state }))
}

/// Put one room's devices (and only the non-default flags) into the two answer maps. An empty room is omitted from
/// both, exactly as before: it cannot be told apart from a room the caller may not see.
fn add_presence(
    rooms: &mut std::collections::BTreeMap<String, Vec<String>>,
    state: &mut std::collections::BTreeMap<String, std::collections::BTreeMap<String, CallPeerState>>,
    gid: &str,
    entries: &[(String, CallPeerState)],
) {
    if entries.is_empty() { return; }
    rooms.insert(gid.to_string(), entries.iter().map(|(d, _)| d.clone()).collect());
    let mut m = std::collections::BTreeMap::new();
    for (d, s) in entries { if !s.is_plain() { m.insert(d.clone(), *s); } }
    if !m.is_empty() { state.insert(gid.to_string(), m); }
}

async fn handle(State(st): State<Arc<RelayState>>, req: Request<Body>) -> Resp {
    let (parts, body) = req.into_parts();
    let path = parts.uri.path().to_string();
    let m = parts.method.clone();
    if m == Method::GET { if let Some(dir) = &st.limits.site_dir { if let Some(r) = site(dir, &path, st.limits.min_app_version.as_deref()).await { return r; } } }
    let limit = body_limit(&m, &path);
    // A slot for a big body goes only to a registered, unrevoked device (the full signature can only be checked once
    // the body is in), and a body that does not arrive in time gives the slot back.
    let _permit = if limit > 4 * 1024 * 1024 {
        let known = {
            let db = st.db.lock().unwrap();
            parts.headers.get(AUTH_HEADER).and_then(|v| v.to_str().ok()).and_then(|h| h.split('.').next()).is_some_and(|d| is_hex(d, 64)
                && matches!(db.query_row("SELECT revoked_sig FROM devices WHERE device=?1", [d], |r| r.get::<_, Option<String>>(0)).optional(), Ok(Some(None))))
        };
        if !known { return status(StatusCode::UNAUTHORIZED, "bad auth"); }
        st.big_bodies.clone().acquire_owned().await.ok()
    } else { None };
    let wait = if limit > 4 * 1024 * 1024 { st.limits.big_body_timeout } else { st.limits.body_timeout };
    let body = match tokio::time::timeout(wait, to_bytes(body, limit)).await {
        Ok(Ok(b)) => b,
        Ok(Err(_)) => return status(StatusCode::PAYLOAD_TOO_LARGE, "too large"),
        Err(_) => return status(StatusCode::REQUEST_TIMEOUT, "request body too slow"),
    };
    // The long-poll endpoint must not hold the DB lock while waiting.
    if m == Method::POST && path == "/v1/sync" {
        return sync(st, &parts.headers, &path, &body).await;
    }
    // The live channel's long-poll must not hold the database lock while it waits either.
    if m == Method::POST && path == "/v1/live/poll" && st.limits.live.enabled {
        return live_poll(st, &parts.headers, &path, &body).await;
    }
    // Asking the media server must not hold the database (it may take a moment), so this runs on the async side.
    if m == Method::POST && path == "/v1/call-presence" {
        return call_presence(st, parts.headers.clone(), &path, &body).await.unwrap_or_else(|r| r);
    }
    let seg: Vec<String> = path.trim_matches('/').split('/').map(String::from).collect();
    // Blob bytes go to and from the disk with no database lock held, and downloads are streamed.
    if seg.len() == 3 && seg[0] == "v1" && seg[1] == "blobs" {
        if m == Method::GET { return get_blob(st, &parts.headers, &path, &seg[2]).await; }
        if m == Method::PUT { return put_blob(st, parts.headers, path.clone(), seg[2].clone(), body).await; }
    }
    // A large-file chunk is one piece, not the whole file: it is read from disk and streamed, and written straight to
    // disk on arrival. Only one chunk is ever in memory, whichever way it is going.
    if seg.len() == 5 && seg[0] == "v1" && seg[1] == "large-files" && seg[3] == "chunks" {
        if m == Method::GET { return get_large_chunk(st, &parts.headers, &path, &seg[2], &seg[4]).await; }
        if m == Method::PUT { return put_large_chunk(st, parts.headers, path.clone(), seg[2].clone(), seg[4].clone(), body).await; }
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

// ---- large files: resumable chunks on disk, one chunk in memory at a time ----

/// The plaintext length of chunk `index`; the stored ciphertext is that plus the 16-byte Poly1305 tag.
fn large_chunk_len(size: u64, chunk_size: u64, chunks: u64, index: u64) -> Option<u64> {
    if chunk_size == 0 || index >= chunks { return None; }
    Some(if index + 1 == chunks { size - chunk_size * (chunks - 1) } else { chunk_size })
}

fn chunk_file(st: &RelayState, id: &str, index: u64) -> PathBuf { st.large_dir.join(id).join(index.to_string()) }

/// Chunk indexes present on disk. A row is written before its directory, so anything listed here belongs to it.
fn uploaded_chunks(st: &RelayState, id: &str, chunks: u64) -> Vec<u64> {
    let mut v: Vec<u64> = std::fs::read_dir(st.large_dir.join(id)).map(|rd| rd.flatten()
        .filter_map(|e| e.file_name().to_string_lossy().parse::<u64>().ok())
        .filter(|i| *i < chunks).collect()).unwrap_or_default();
    v.sort_unstable();
    v
}

/// Remove every large file whose time is up. Called from the background sweeper and before a new reservation.
fn sweep_large_locked(db: &Connection, st: &RelayState) {
    let now = now_secs() as i64;
    let ids: Vec<String> = match db.prepare("SELECT id FROM large_files WHERE expires<=?1") {
        Ok(mut s) => s.query_map([now], |r| r.get(0)).map(|rows| rows.flatten().collect()).unwrap_or_default(),
        Err(_) => return,
    };
    for id in ids {
        if db.execute("DELETE FROM large_files WHERE id=?1", [&id]).is_ok() { let _ = std::fs::remove_dir_all(st.large_dir.join(&id)); }
    }
}

async fn get_large_chunk(st: Arc<RelayState>, headers: &HeaderMap, path: &str, id: &str, index: &str) -> Resp {
    if !is_hex(id, 32) { return status(StatusCode::BAD_REQUEST, "bad id"); }
    let Ok(idx) = index.parse::<u64>() else { return status(StatusCode::BAD_REQUEST, "bad chunk") };
    {
        let db = st.db.lock().unwrap();
        let a = match authenticate(&db, headers, "GET", path, &[], true) { Ok((_, Some(a))) => a, Ok(_) => return status(StatusCode::FORBIDDEN, "x"), Err(r) => return r };
        let row: Option<(String, i64, i64, i64, i64, i64)> = db.query_row(
            "SELECT gid,size,chunk_size,chunks,complete,expires FROM large_files WHERE id=?1", [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).optional()
            .unwrap_or(None);
        let Some((gid, size, cs, chunks, complete, expires)) = row else { return status(StatusCode::NOT_FOUND, "no such file") };
        if expires as u64 <= now_secs() { return status(StatusCode::NOT_FOUND, "this file has expired") }
        if complete == 0 { return status(StatusCode::CONFLICT, "this file is still being uploaded") }
        match is_member(&db, &gid, &a.account) { Ok(true) => {}, Ok(false) => return status(StatusCode::FORBIDDEN, "not a member"), Err(r) => return r }
        if large_chunk_len(size as u64, cs as u64, chunks as u64, idx).is_none() { return status(StatusCode::NOT_FOUND, "no such chunk") }
    }
    let Ok(f) = tokio::fs::File::open(chunk_file(&st, id, idx)).await else { return status(StatusCode::NOT_FOUND, "no such chunk") };
    let len = f.metadata().await.map(|m| m.len()).unwrap_or(0);
    Response::builder().status(200).header("content-type", "application/octet-stream").header("content-length", len)
        .body(Body::from_stream(tokio_util::io::ReaderStream::new(f))).unwrap()
}

async fn put_large_chunk(st: Arc<RelayState>, headers: HeaderMap, path: String, id: String, index: String, body: axum::body::Bytes) -> Resp {
    let task = tokio::task::spawn_blocking(move || -> Result<Resp, Resp> {
        let Ok(idx) = index.parse::<u64>() else { return Err(status(StatusCode::BAD_REQUEST, "bad chunk")) };
        let (size, chunk_size, chunks) = {
            let db = st.db.lock().unwrap();
            let a = authenticate(&db, &headers, "PUT", &path, &body, true)?.1.ok_or_else(|| status(StatusCode::FORBIDDEN, "x"))?;
            if !is_hex(&id, 32) { return Err(status(StatusCode::BAD_REQUEST, "bad id")); }
            let row: Option<(String, String, i64, i64, i64, i64)> = db.query_row(
                "SELECT account,gid,size,chunk_size,chunks,complete FROM large_files WHERE id=?1", [&id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).optional().map_err(dberr)?;
            let Some((account, _gid, size, cs, chunks, complete)) = row else { return Err(status(StatusCode::NOT_FOUND, "no such file")) };
            if account != a.account { return Err(status(StatusCode::FORBIDDEN, "not your upload")) }
            if complete != 0 { return Err(status(StatusCode::CONFLICT, "already finished")) }
            (size as u64, cs as u64, chunks as u64)
        };
        let Some(plain) = large_chunk_len(size, chunk_size, chunks, idx) else { return Err(status(StatusCode::NOT_FOUND, "no such chunk")) };
        // The exact ciphertext length is known from the announcement, so junk is refused before it reaches the disk.
        if body.len() as u64 != plain + 16 { return Err(status(StatusCode::BAD_REQUEST, "bad chunk length")); }
        let dir = st.large_dir.join(&id);
        std::fs::create_dir_all(&dir).map_err(dberr)?;
        let (tmp, p) = (dir.join(format!(".{idx}.tmp")), dir.join(idx.to_string()));
        if let Err(e) = std::fs::write(&tmp, &body) {
            let _ = std::fs::remove_file(&tmp);
            return Err(dberr(e));
        }
        let _ = std::fs::remove_file(&p); // Windows does not rename over an existing file
        std::fs::rename(&tmp, &p).map_err(dberr)?;
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
            st.limited("info".into(), lim.info_checks_per_window, "too many attempts, try again later")?;
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
            st.limited("revoke".into(), lim.revokes_per_window, "too many revocations, try again later")?;
            verify_revocation(&r.account, &r.device, &r.sig).map_err(|_| status(StatusCode::BAD_REQUEST, "bad sig"))?;
            // Only a real, not-yet-revoked device of that account loses its key packages and Welcomes; a signature
            // over some other key must not wipe anything.
            let changed = db.execute("UPDATE devices SET revoked_sig=?1 WHERE device=?2 AND account=?3 AND revoked_sig IS NULL", params![r.sig, r.device, r.account]).map_err(dberr)?;
            if changed > 0 {
                db.execute("DELETE FROM keypackages WHERE device=?1", [&r.device]).map_err(dberr)?;
                db.execute("DELETE FROM welcomes WHERE device=?1", [&r.device]).map_err(dberr)?;
            }
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
        // ---- the live channel: sealed events for the people in the room right now, held in memory and never stored ----
        (Method::POST, ["v1", "groups", gid, "live"]) if lim.live.enabled => {
            let a = authenticate(&db, headers, ms, path, body, true)?.1.unwrap();
            if !valid_gid(gid) { return Err(status(StatusCode::BAD_REQUEST, "bad group")); }
            let r: LivePostReq = parse(body)?;
            if r.events.is_empty() || r.events.len() > LIVE_POST_MAX { return Err(status(StatusCode::BAD_REQUEST, "1 to 20 events at a time")); }
            if !is_member(&db, gid, &a.account)? { return Err(status(StatusCode::FORBIDDEN, "not a member")); }
            drop(db);
            let mut blobs = Vec::with_capacity(r.events.len());
            for e in &r.events { blobs.push(dec(e, LIVE_MAX_BLOB)?); }
            let (mut taken, mut limited, mut full) = (0u32, 0u32, 0u32);
            {
                let now = st.live_now();
                let mut hub = st.live.lock().unwrap();
                for b in blobs {
                    match hub.push(gid, &a.device, b, now) {
                        live::Push::Taken => taken += 1,
                        live::Push::RateLimited => limited += 1,
                        live::Push::Full => full += 1,
                        live::Push::TooBig => return Err(status(StatusCode::PAYLOAD_TOO_LARGE, "too large")),
                    }
                }
            }
            if taken > 0 { st.live_tick.send_modify(|v| *v += 1); }
            if taken == 0 && limited > 0 { return Err(status(StatusCode::TOO_MANY_REQUESTS, "too many live events, slow down")); }
            if taken == 0 && full > 0 { return Err(status(StatusCode::SERVICE_UNAVAILABLE, "the live buffer is full for now")); }
            Ok(json(&LivePostResp { accepted: taken }))
        }
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
            Ok(json(&CallTokenResp { url: cfg.url.clone(), token, room, identity: a.device, expires, turn_port: cfg.turn_port }))
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
        // ---- large files: own quota, own expiry, chunks on disk ----
        (Method::POST, ["v1", "large-files", id, "begin"]) => {
            let a = authenticate(&db, headers, ms, path, body, true)?.1.ok_or_else(|| status(StatusCode::FORBIDDEN, "x"))?;
            if !is_hex(id, 32) { return Err(status(StatusCode::BAD_REQUEST, "bad id")); }
            let r: LargeBeginReq = parse(body)?;
            if !valid_gid(&r.gid) { return Err(status(StatusCode::BAD_REQUEST, "bad group")); }
            if r.size == 0 || r.size > lim.large_file_max_bytes { return Err(status(StatusCode::BAD_REQUEST, "bad file size")); }
            if r.chunk_size < MIN_LARGE_CHUNK_BYTES || r.chunk_size > MAX_LARGE_CHUNK_BYTES { return Err(status(StatusCode::BAD_REQUEST, "bad chunk size")); }
            let chunks = r.size / r.chunk_size + u64::from(r.size % r.chunk_size != 0);
            if chunks == 0 || chunks > 1_000_000 { return Err(status(StatusCode::BAD_REQUEST, "too many chunks")); }
            if !is_member(&db, &r.gid, &a.account)? { return Err(status(StatusCode::FORBIDDEN, "not a member")); }
            let auth = group_auth(&db, &r.gid)?.ok_or_else(|| status(StatusCode::NOT_FOUND, "unknown community"))?;
            st.limited(format!("large|{}", a.device), lim.large_begins_per_window, "too many large uploads, wait a little")?;
            sweep_large_locked(&db, st);
            let now = now_secs() as i64;
            let existing: Option<(String, String, i64, i64, i64, i64, i64)> = db.query_row(
                "SELECT account,gid,size,chunk_size,chunks,complete,expires FROM large_files WHERE id=?1", [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))).optional().map_err(dberr)?;
            if let Some((account, gid, size, cs, have_chunks, complete, expires)) = existing {
                if account != a.account || gid != r.gid || size as u64 != r.size || cs as u64 != r.chunk_size || have_chunks as u64 != chunks {
                    return Err(status(StatusCode::CONFLICT, "that file id belongs to a different file"));
                }
                let uploaded = uploaded_chunks(st, id, chunks);
                return Ok(json(&LargeInfoResp { size: r.size, chunk_size: r.chunk_size, chunks, complete: complete != 0, expires: expires as u64, uploaded }));
            }
            let used: i64 = db.query_row("SELECT COALESCE(SUM(size),0) FROM large_files WHERE account=?1", [&a.account], |r| r.get(0)).map_err(dberr)?;
            let quota = if a.account == auth.owner { lim.large_file_quota_owner } else { lim.large_file_quota_per_account };
            if used as u64 + r.size > quota { return Err(status(StatusCode::INSUFFICIENT_STORAGE, "large-file storage quota reached")); }
            let total: i64 = db.query_row("SELECT COALESCE(SUM(size),0) FROM large_files", [], |r| r.get(0)).map_err(dberr)?;
            if total as u64 + r.size > lim.large_file_total { return Err(status(StatusCode::INSUFFICIENT_STORAGE, "the relay's large-file area is full")); }
            let expires = now + lim.large_file_expire_secs as i64;
            db.execute("INSERT INTO large_files(id,account,gid,size,chunk_size,chunks,complete,expires,created) VALUES(?1,?2,?3,?4,?5,?6,0,?7,?8)",
                params![id, a.account, r.gid, r.size as i64, r.chunk_size as i64, chunks as i64, expires, now]).map_err(dberr)?;
            if let Err(e) = std::fs::create_dir_all(st.large_dir.join(id)) {
                let _ = db.execute("DELETE FROM large_files WHERE id=?1", [id]);
                return Err(dberr(e));
            }
            Ok(json(&LargeInfoResp { size: r.size, chunk_size: r.chunk_size, chunks, complete: false, expires: expires as u64, uploaded: vec![] }))
        }
        (Method::GET, ["v1", "large-files", id, "info"]) => {
            let a = authenticate(&db, headers, ms, path, body, true)?.1.ok_or_else(|| status(StatusCode::FORBIDDEN, "x"))?;
            if !is_hex(id, 32) { return Err(status(StatusCode::BAD_REQUEST, "bad id")); }
            let row: Option<(String, String, i64, i64, i64, i64, i64)> = db.query_row(
                "SELECT account,gid,size,chunk_size,chunks,complete,expires FROM large_files WHERE id=?1", [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))).optional().map_err(dberr)?;
            let Some((account, gid, size, cs, chunks, complete, expires)) = row else { return Err(status(StatusCode::NOT_FOUND, "no such file")) };
            if expires as u64 <= now_secs() { return Err(status(StatusCode::NOT_FOUND, "this file has expired")); }
            if account != a.account && !is_member(&db, &gid, &a.account)? { return Err(status(StatusCode::FORBIDDEN, "not a member")); }
            let uploaded = uploaded_chunks(st, id, chunks as u64);
            Ok(json(&LargeInfoResp { size: size as u64, chunk_size: cs as u64, chunks: chunks as u64, complete: complete != 0, expires: expires as u64, uploaded }))
        }
        (Method::POST, ["v1", "large-files", id, "finish"]) => {
            let a = authenticate(&db, headers, ms, path, body, true)?.1.ok_or_else(|| status(StatusCode::FORBIDDEN, "x"))?;
            if !is_hex(id, 32) { return Err(status(StatusCode::BAD_REQUEST, "bad id")); }
            let r: LargeFinishReq = parse(body)?;
            if !is_hex(&r.sha256, 64) { return Err(status(StatusCode::BAD_REQUEST, "bad hash")); }
            let row: Option<(String, i64, i64, i64, i64, i64)> = db.query_row(
                "SELECT account,size,chunk_size,chunks,complete,expires FROM large_files WHERE id=?1", [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).optional().map_err(dberr)?;
            let Some((account, size, cs, chunks, complete, expires)) = row else { return Err(status(StatusCode::NOT_FOUND, "no such file")) };
            if expires as u64 <= now_secs() { return Err(status(StatusCode::NOT_FOUND, "this file has expired")); }
            if account != a.account { return Err(status(StatusCode::FORBIDDEN, "not your upload")); }
            if complete == 0 {
                for i in 0..chunks as u64 {
                    let want = large_chunk_len(size as u64, cs as u64, chunks as u64, i).unwrap_or(0) + 16;
                    if std::fs::metadata(chunk_file(st, id, i)).map(|m| m.len() != want).unwrap_or(true) {
                        return Err(status(StatusCode::CONFLICT, "chunks are missing; send them again"));
                    }
                }
                db.execute("UPDATE large_files SET complete=1, sha256=?1 WHERE id=?2", params![r.sha256, id]).map_err(dberr)?;
            }
            Ok(json(&LargeInfoResp { size: size as u64, chunk_size: cs as u64, chunks: chunks as u64, complete: true, expires: expires as u64, uploaded: (0..chunks as u64).collect() }))
        }
        (Method::DELETE, ["v1", "large-files", id]) => {
            let a = authenticate(&db, headers, ms, path, body, true)?.1.ok_or_else(|| status(StatusCode::FORBIDDEN, "x"))?;
            if !is_hex(id, 32) { return Err(status(StatusCode::BAD_REQUEST, "bad id")); }
            let account: Option<String> = db.query_row("SELECT account FROM large_files WHERE id=?1", [id], |r| r.get(0)).optional().map_err(dberr)?;
            match account {
                Some(owner) if owner == a.account => {}
                Some(_) => return Err(status(StatusCode::FORBIDDEN, "only the uploader can delete this file")),
                None => return Ok(status(StatusCode::OK, "ok")),
            }
            db.execute("DELETE FROM large_files WHERE id=?1", [id]).map_err(dberr)?;
            let _ = std::fs::remove_dir_all(st.large_dir.join(id));
            Ok(status(StatusCode::OK, "ok"))
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

/// Which of these rooms the account belongs to right now (as the relay's own list says).
fn live_member_rooms(st: &RelayState, account: &str, gids: impl Iterator<Item = String>) -> Result<BTreeSet<String>, Resp> {
    let db = st.db.lock().unwrap();
    let mut out = BTreeSet::new();
    for g in gids { if valid_gid(&g) && is_member(&db, &g, account)? { out.insert(g); } }
    Ok(out)
}

/// `POST /v1/live/poll`: what is new in the caller's rooms, or wait for it. Only the rooms the caller belongs to are
/// answered; the others are left out without a word. A cursor from another relay run is not trusted: the answer then
/// holds no events, only where the room is now.
async fn live_poll(st: Arc<RelayState>, headers: &HeaderMap, path: &str, body: &[u8]) -> Resp {
    let a = {
        let db = st.db.lock().unwrap();
        match authenticate(&db, headers, "POST", path, body, true) { Ok((_, Some(a))) => a, Ok(_) => return status(StatusCode::FORBIDDEN, "x"), Err(r) => return r }
    };
    let req: LivePollReq = match parse(body) { Ok(r) => r, Err(r) => return r };
    if req.cursors.len() > 500 { return status(StatusCode::PAYLOAD_TOO_LARGE, "too many rooms"); }
    let rooms = match live_member_rooms(&st, &a.account, req.cursors.keys().cloned()) { Ok(r) => r, Err(r) => return r };
    let deadline = tokio::time::Instant::now() + Duration::from_millis(req.wait_ms.min(25_000));
    let mut rx = st.live_tick.subscribe();
    loop {
        rx.borrow_and_update();
        let mut out = LivePollResp::default();
        let mut any = false;
        {
            let now = st.live_now();
            let mut hub = st.live.lock().unwrap();
            out.boot = hub.boot.clone();
            let same = req.boot == hub.boot;
            for (gid, after) in &req.cursors {
                if !rooms.contains(gid) { continue; }
                if !same {
                    // Tell the caller where the room stands now, so it can follow from here.
                    out.groups.insert(gid.clone(), LiveGroupResp { next: hub.head_of(gid), events: vec![] });
                    continue;
                }
                let (evs, next) = hub.since(gid, *after, &a.device, now);
                // A room the caller has no position in yet (u64::MAX) is answered at once with where it stands: waiting would
                // only delay the caller's real polls.
                any |= !evs.is_empty() || *after == u64::MAX;
                out.groups.insert(gid.clone(), LiveGroupResp { next, events: evs.into_iter().map(|(seq, from, data)| LiveItem { seq, from, data: b64(&data) }).collect() });
            }
        }
        if any || req.wait_ms == 0 || tokio::time::Instant::now() >= deadline || out.boot != req.boot {
            // Membership may have changed while we waited: events go only to current members.
            if any {
                let still = match live_member_rooms(&st, &a.account, out.groups.keys().cloned()) { Ok(r) => r, Err(r) => return r };
                out.groups.retain(|g, _| still.contains(g));
            }
            return json(&out);
        }
        let _ = tokio::time::timeout_at(deadline, rx.changed()).await;
    }
}

#[cfg(test)]
mod site_tests {
    use super::*;
    async fn body(r: Resp) -> (u16, String) { let c = r.status().as_u16(); (c, String::from_utf8_lossy(&to_bytes(r.into_body(), 1 << 20).await.unwrap()).to_string()) }
    fn tmp() -> PathBuf { let d = std::env::temp_dir().join(format!("commons-site-{}", rand::thread_rng().gen::<u64>())); std::fs::create_dir_all(d.join("update")).unwrap(); d }

    #[tokio::test]
    async fn updates_are_served_from_their_own_folder_and_not_listed_for_friends() {
        let d = tmp();
        std::fs::write(d.join("Commons-linux-x86_64.AppImage"), b"app").unwrap();
        std::fs::write(d.join("Megatron-windows-browser.zip"), b"win browser").unwrap();
        std::fs::write(d.join("Megatron-macos-browser.zip"), b"mac browser").unwrap();
        std::fs::write(d.join("update/latest.json"), br#"{"version":"0.2.1","platforms":{"linux-x86_64":{"url":"update/Commons_0.2.1_amd64.AppImage","signature":"c2ln"}}}"#).unwrap();
        std::fs::write(d.join("update/Commons_0.2.1_amd64.AppImage"), b"new app").unwrap();
        let (c, list) = body(site(&d, "/download/", None).await.unwrap()).await;
        assert_eq!(c, 200);
        assert!(list.contains("Commons-linux-x86_64.AppImage") && !list.contains("latest.json") && !list.contains("0.2.1"), "the download page lists only what friends install: {list}");
        assert!(list.contains("Megatron-windows-browser.zip") && list.contains("Megatron-macos-browser.zip"), "both browser versions are offered: {list}");
        let (c, m) = body(site(&d, "/update/latest.json", None).await.unwrap()).await;
        assert_eq!(c, 200);
        let v: serde_json::Value = serde_json::from_str(&m).unwrap();
        assert_eq!(v["version"], "0.2.1");
        assert!(v.get("minimum_version").is_none());
        assert_eq!(body(site(&d, "/update/Commons_0.2.1_amd64.AppImage", None).await.unwrap()).await, (200, "new app".into()));
        for bad in ["/update/../Commons-linux-x86_64.AppImage", "/update/.hidden", "/update/a/b", "/update/", "/update/missing"] {
            assert_eq!(body(site(&d, bad, None).await.unwrap()).await.0, 404, "{bad}");
        }
        std::fs::remove_dir_all(&d).ok();
    }

    #[tokio::test]
    async fn the_relay_can_set_a_minimum_version() {
        let d = tmp();
        std::fs::write(d.join("update/latest.json"), br#"{"version":"0.3.0","minimum_version":"0.1.0","platforms":{}}"#).unwrap();
        let (_, m) = body(site(&d, "/update/latest.json", Some("0.2.0")).await.unwrap()).await;
        let v: serde_json::Value = serde_json::from_str(&m).unwrap();
        assert_eq!(v["minimum_version"], "0.2.0", "the relay's setting wins over the published one");
        assert_eq!(v["version"], "0.3.0");
        std::fs::write(d.join("update/latest.json"), b"not json").unwrap();
        assert_eq!(body(site(&d, "/update/latest.json", Some("0.2.0")).await.unwrap()).await.0, 404);
        std::fs::remove_dir_all(&d).ok();
        let none = std::env::temp_dir().join("commons-site-none-xyz");
        assert_eq!(body(site(&none, "/update/latest.json", None).await.unwrap()).await.0, 404, "nothing published: a plain 404, which the app reads as no updates");
    }

    #[test]
    fn versions_the_relay_accepts() {
        for ok in ["0.2.0", "10.20.30", "1.0.0-rc.1"] { assert!(is_app_version(ok), "{ok}"); }
        for bad in ["", "0.2", "0.2.0.1", "v0.2.0", "0.2.x", "0.2.0-", "0.2.0-a b", "1.2.3-;"] { assert!(!is_app_version(bad), "{bad}"); }
    }
}
