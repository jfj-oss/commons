//! JSON dispatch used by both the Tauri shell and the local dev bridge: the UI talks to the Rust core only
//! through `App::call(method, args)`. Keys and plaintext never leave this process.
use crate::client::*;
use crate::envelope::{check_name, safe_file_name, InviteLink, RelayLink};
use crate::state::*;
use crate::transport::{Api, HttpTransport};
use crate::vault::KdfParams;
use commons_proto::{b64, unb64, LargeBeginReq};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, TryLockError};
use std::time::{Duration, Instant};

/// Decrypted attachments kept for the chunked reads of `file_read`: a few, and only while unlocked.
const OPEN_FILES: usize = 4;
const FILE_CHUNK: usize = 1 << 20;

pub struct App {
    dir: PathBuf,
    default_relay: String,
    kdf: KdfParams,
    client: Mutex<Option<Client>>,
    /// Pairing in progress (new device waiting for approval).
    wake: (Mutex<bool>, Condvar),
    /// Wakes the live channel's sender when something is waiting to go out.
    live_wake: (Mutex<bool>, Condvar),
    stop: Mutex<bool>,
    /// The last answer to each read-only call. The UI asks twice a second, and gets this if the client is busy.
    seen: Mutex<HashMap<String, (Value, u64, Instant)>>,
    /// How many times the client has been held for a change (an action from the UI, a sync round, the live channel):
    /// a cached answer from before the latest is not served as the state of now (`read`).
    changes: AtomicU64,
    /// Per `messages` key, the last few windows served: each answer's hash with the id and hash of every row in it, so a
    /// page that says which answer it holds can be sent only what changed (`messages_answer`).
    windows: Mutex<HashMap<String, std::collections::VecDeque<(String, Vec<(String, u64)>)>>>,
    files: Mutex<Vec<(String, Arc<Vec<u8>>)>>,
    /// loopback server for "open this call in my browser" (started on first use). Desktops only: a phone has no
    /// second browser to hand a call to.
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    bridge: Mutex<Option<crate::callbridge::CallBridge>>,
    /// Per-game push-to-mute. Runs only while a call keeps asking.
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    game: commons_gamekeys::Service,
    /// When a call last asked the core for something (its key poll, token, or game-key heartbeat), from this window or
    /// from a call handed off to the browser. The updater waits while this is recent: `call_active`.
    call_seen: Mutex<Option<std::time::Instant>>,
    /// The invite link a password just fetched, kept for a few minutes. A fetch spends one of the invite's few; if what comes next
    /// fails (a relay that wants its secret, a full relay, a lost connection), trying again must not spend another.
    pw_link: Mutex<Option<(String, String, std::time::Instant)>>,
    me: std::sync::Weak<App>,
    /// Notification rules and hand-over between the page and the background part (notify.rs).
    notify: Arc<crate::notify::Hub>,
}

/// A running call asks the core at least every few seconds (call.js: the key poll every 1.5 s, the game-key heartbeat
/// at most 4 s apart), so this much silence means no call is running on this device.
pub const CALL_QUIET: Duration = Duration::from_secs(20);

/// How often the core looks at rune while sharing is on. This is not the send rate: a shared status goes out only
/// when it changes, plus a renewal every `LISTENING_REFRESH_SECS` while it keeps playing.
const RUNE_POLL_SECS: u64 = 15;

/// How long the live poller lets the relay hold a poll for news (the relay caps it at 25 s). A room joined meanwhile is
/// not in the poll in flight; the ordinary round polls such a room until the next poll covers it (round.rs).
const LIVE_POLL_WAIT_MS: u64 = 20_000;

/// How long a read waits for a busy client when its cached answer is from before the UI's last action (`App::read`).
const READ_WAIT: Duration = Duration::from_millis(1500);

fn s(v: &Value, k: &str) -> String { v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string() }

/// COMMONS_LOCK_TRACE set: say on stderr when the client is waited for or held for long, and when a read is answered from the cache.
fn trace_on() -> bool { static ON: OnceLock<bool> = OnceLock::new(); *ON.get_or_init(|| std::env::var_os("COMMONS_LOCK_TRACE").is_some()) }
const TRACE_MS: u128 = 100;

/// The client, locked. Knows who took it and for how long (for the trace), and counts itself as a change when let go
/// (`changes`, None for a read: it changes nothing). The count goes up before the client is free, so a read that then
/// gets it sees the new count with the new state.
pub struct Held<'a> { g: MutexGuard<'a, Option<Client>>, at: Instant, waited: Duration, who: &'static std::panic::Location<'static>, changes: Option<&'a AtomicU64> }
impl std::ops::Deref for Held<'_> { type Target = Option<Client>; fn deref(&self) -> &Option<Client> { &self.g } }
impl std::ops::DerefMut for Held<'_> { fn deref_mut(&mut self) -> &mut Option<Client> { &mut self.g } }
impl Drop for Held<'_> {
    fn drop(&mut self) {
        if let Some(c) = self.changes { c.fetch_add(1, Ordering::SeqCst); }
        if !trace_on() { return; }
        let held = self.at.elapsed().as_millis();
        if held >= TRACE_MS || self.waited.as_millis() >= TRACE_MS { eprintln!("[lock] {}:{} waited {}ms held {}ms", self.who.file(), self.who.line(), self.waited.as_millis(), held); }
    }
}
type R = Result<Value, String>;
fn e<T: std::fmt::Display>(x: T) -> String { format!("{x}") }
/// [`e`] for a relay call during a large transfer: a dropped connection, a timeout, a relay that is restarting or busy
/// is marked `[retry] ` so the page waits and tries again; anything else (refused, expired, over quota) is final.
fn xe(x: anyhow::Error) -> String {
    let transient = match crate::transport::relay_status(&x) {
        Some(st) => matches!(st, 408 | 425 | 429 | 500..=599),
        None => x.to_string().starts_with("network") || x.is::<std::io::Error>(),
    };
    if transient { format!("[retry] {x}") } else { format!("{x}") }
}

/// A usable relay address, in the one spelling we store: `http(s)://host[:port][/path]`, no trailing slash.
pub fn clean_relay_url(u: &str) -> Result<String, String> {
    let u = u.trim().trim_end_matches('/');
    let bad = || format!("\"{u}\" is not a relay address. It should look like https://chat.example.org or http://127.0.0.1:8787");
    let rest = u.strip_prefix("https://").or_else(|| u.strip_prefix("http://")).ok_or_else(bad)?;
    let host = rest.split('/').next().unwrap_or("");
    let name = host.rsplit_once(':').filter(|(_, p)| p.parse::<u16>().is_ok()).map(|(h, _)| h).unwrap_or(host);
    let ok = !name.is_empty() && !name.contains(':') && name.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_')) && !rest.contains(['?', '#', '@', ' ', '\\']);
    if ok { Ok(u.to_string()) } else { Err(bad()) }
}

/// What a person can do about a relay that does not answer. The technical reason stays at the end, for whoever helps them.
pub fn unreachable_text(relay: &str, why: &str) -> String {
    let w = why.to_ascii_lowercase();
    let hint = if w.contains("not a megatron relay") || w.contains("relay said") { "Something answers at that address, but it is not a Megatron relay. Check the address, including the port." }
        else if w.contains("dns") || w.contains("resolve") || w.contains("lookup") || w.contains("name or service") { "That address could not be found. Check it for typos, and check that this computer is online." }
        else if w.contains("certificate") || w.contains("tls") || w.contains("handshake") { "The relay's secure connection could not be checked. Check that this computer's date and time are right; if the address is right too, tell whoever runs it." }
        else if w.contains("refused") { "Nothing is answering there. The relay is probably switched off: ask whoever runs it to start it." }
        else if w.contains("timed out") || w.contains("timeout") { "It did not answer in time. The relay may be switched off, or this network may be blocking it." }
        else { "The relay may be switched off, or the address may be wrong." };
    format!("Could not reach the relay at {relay}. {hint} ({why})")
}

/// The dev bridge has no password, so nothing but a page it served itself may reach it. Host stops DNS rebinding,
/// where a hostile site's name is pointed at 127.0.0.1; Origin stops a hostile page from posting to the loopback port.
pub fn bridge_request_ok(host: Option<&str>, origin: Option<&str>, port: u16) -> bool {
    let ours = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    let host_ok = host.is_some_and(|h| ours.iter().any(|o| o == h));
    let origin_ok = origin.is_none_or(|o| ours.iter().any(|x| o == format!("http://{x}")));
    host_ok && origin_ok
}

impl App {
    pub fn new(dir: PathBuf, default_relay: &str) -> Arc<App> { Self::with_kdf(dir, default_relay, KdfParams::STRONG) }
    pub fn with_kdf(dir: PathBuf, default_relay: &str, kdf: KdfParams) -> Arc<App> {
        let notify = Arc::new(crate::notify::Hub::new(Some(dir.clone())));
        Arc::new_cyclic(|me| App {
            notify, me: me.clone(), call_seen: Mutex::new(None), pw_link: Mutex::new(None), dir, default_relay: default_relay.into(), kdf, client: Mutex::new(None),
            wake: (Mutex::new(false), Condvar::new()), live_wake: (Mutex::new(false), Condvar::new()), stop: Mutex::new(false), seen: Mutex::new(HashMap::new()), changes: AtomicU64::new(0), windows: Mutex::new(HashMap::new()), files: Mutex::new(vec![]),
            #[cfg(not(any(target_os = "ios", target_os = "android")))]
            bridge: Mutex::new(None),
            #[cfg(not(any(target_os = "ios", target_os = "android")))]
            game: commons_gamekeys::Service::new(),
        })
    }
    /// The notification hub: the program that owns this core installs how notes are shown (`Hub::set_sink`).
    pub fn notify_hub(&self) -> Arc<crate::notify::Hub> { self.notify.clone() }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    fn open_call_in_browser(&self, gid: &str, name: &str, launch: bool) -> R {
        self.with(|c| c.call_state(gid).map(|_| ()).map_err(e))?;      // only for rooms we are really in
        let me = self.me.upgrade().ok_or("app is shutting down")?;
        let url = {
            let mut g = self.bridge.lock().unwrap();
            if g.is_none() { *g = Some(crate::callbridge::CallBridge::start(me).map_err(e)?); }
            g.as_ref().unwrap().grant(gid, name)
        };
        let opened = launch && crate::callbridge::open_in_browser(&url);
        Ok(json!({"url": url, "opened": opened}))
    }
    pub fn shutdown(&self) {
        *self.stop.lock().unwrap() = true;
        self.nudge();
        crate::pcaudio::shutdown();   // an app's sound being shared goes back where it was
        // The browser bridge's listener thread holds the app alive, so it has to be ended here.
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let bridge = self.bridge.lock().unwrap().take();
            if let Some(b) = bridge { b.stop(); }
        }
    }
    fn saw_call(&self) { *self.call_seen.lock().unwrap_or_else(|p| p.into_inner()) = Some(std::time::Instant::now()); }
    /// True while a call on this device is running (in this window or in the browser): an update must not restart the app now.
    pub fn call_active(&self) -> bool {
        self.call_seen.lock().unwrap_or_else(|p| p.into_inner()).is_some_and(|t| t.elapsed() < CALL_QUIET)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    #[doc(hidden)]
    pub fn bridge_running(&self) -> bool { self.bridge.lock().unwrap().as_ref().is_some_and(|b| !b.is_stopped()) }
    fn nudge(&self) {
        *self.wake.0.lock().unwrap() = true; self.wake.1.notify_all();
        *self.live_wake.0.lock().unwrap() = true; self.live_wake.1.notify_all();
    }
    /// A panic in one call must not take every later call down with it.
    #[track_caller]
    fn lock(&self) -> Held<'_> { self.lock_at(std::panic::Location::caller()) }
    fn lock_at(&self, who: &'static std::panic::Location<'static>) -> Held<'_> {
        let t = Instant::now();
        let g = self.client.lock().unwrap_or_else(|p| p.into_inner());
        Held { g, at: Instant::now(), waited: t.elapsed(), who, changes: Some(&self.changes) }
    }

    /// Background loop: a round every second (or immediately after a user action, or when the sync poller brought news).
    /// The client is held only to plan a round and to apply its result, never while waiting for the relay. Receiving is
    /// the sync poller's below; the round sends (and syncs only for a group the poller's request in flight does not
    /// cover), so what this device sends never waits behind a request the relay is holding.
    pub fn start_background(self: &Arc<Self>) {
        let app = self.clone();
        std::thread::spawn(move || loop {
            {
                let g = app.wake.0.lock().unwrap();
                let _ = app.wake.1.wait_timeout_while(g, Duration::from_millis(1000), |w| !*w).unwrap();
                *app.wake.0.lock().unwrap() = false;
            }
            if *app.stop.lock().unwrap() { break; }
            for _ in 0..16 {
                let Some(round) = app.lock().as_mut().map(|c| c.begin_round(0)) else { break };
                let done = round.run();
                let mut g = app.lock();
                let Some(c) = g.as_mut() else { break };
                let _ = c.finish_round(done);
                if !c.more { break; }
            }
        });
        // The sync poller: one request at a time, which the relay holds until there is something for this device or
        // SYNC_POLL_WAIT_MS pass, so news arrives when it is posted and an idle device costs the relay a request every
        // 25 s instead of every second. Nothing is held while it waits. A relay from before long polls answers at once
        // (the request is the same, with a wait it ignores), which is the old cadence.
        let app = self.clone();
        std::thread::spawn(move || {
            let mut wait = crate::round::SYNC_POLL_WAIT_MS;
            loop {
                if *app.stop.lock().unwrap() { break; }
                let plan = { let mut g = app.lock(); g.as_mut().and_then(|c| c.sync_plan(wait)) };
                match plan {
                    Some(p) => {
                        let d = p.run();
                        let (more, failed, news) = (d.more(), d.failed(), d.got_any());
                        if let Some(c) = app.lock().as_mut() { c.finish_sync(d); }
                        if news { app.nudge(); }   // what came in may want an answer (a welcome, a join to grant, keys to fetch)
                        wait = if more { 0 } else { crate::round::SYNC_POLL_WAIT_MS };
                        if failed { std::thread::sleep(Duration::from_millis(1500)); }
                    }
                    None => std::thread::sleep(Duration::from_millis(500)),
                }
            }
        });
        // The unstored live channel (live.rs) has two threads of its own. The sender posts what is waiting within a tenth
        // of a second; the poller waits at the relay for news, so an event reaches us as soon as it is posted. Neither
        // holds the client while it waits. The ordinary round then only sends, so a relay without the channel is
        // still served by the log, exactly as before.
        let app = self.clone();
        std::thread::spawn(move || loop {
            if *app.stop.lock().unwrap() { break; }
            let plan = { let mut g = app.lock(); g.as_mut().and_then(|c| { c.set_live_background(true); c.flush_live(); c.live_plan_with(true, false) }) };
            match plan {
                Some(p) => { let d = p.run(0); if let Some(c) = app.lock().as_mut() { c.live_finish(d); } }
                None => {
                    let g = app.live_wake.0.lock().unwrap();
                    let mut g = app.live_wake.1.wait_timeout_while(g, Duration::from_millis(100), |w| !*w).unwrap().0;
                    *g = false;
                }
            }
        });
        let app = self.clone();
        std::thread::spawn(move || loop {
            if *app.stop.lock().unwrap() { break; }
            let plan = { let mut g = app.lock(); g.as_mut().and_then(|c| { c.set_live_background(true); let p = c.live_plan_with(false, true); c.set_live_polling(p.as_ref().map(|p| p.rooms()).unwrap_or_default()); p }) };
            match plan {
                Some(p) => {
                    let wait = if p.quick() { 400 } else { LIVE_POLL_WAIT_MS };
                    let d = p.run(wait);
                    let failed = d.failed();
                    if let Some(c) = app.lock().as_mut() { c.live_finish(d); }
                    if failed { std::thread::sleep(Duration::from_millis(1500)); }
                }
                None => std::thread::sleep(Duration::from_millis(500)),
            }
        });
        // New messages while no window is being looked at (notify.rs). Does nothing until the program installs a sink.
        let app = self.clone();
        std::thread::spawn(move || loop {
            if *app.stop.lock().unwrap() { break; }
            std::thread::sleep(Duration::from_millis(1000));
            if !app.notify.has_sink() { continue; }
            // A look, not a change: if the client is busy it is simply looked at a second later.
            let g = match app.client.try_lock() { Ok(g) => g, Err(TryLockError::Poisoned(p)) => p.into_inner(), Err(TryLockError::WouldBlock) => continue };
            if let Some(c) = g.as_ref() { app.notify.tick(c); }
        });
        // Its own thread, because asking rune is a network wait that must not hold up the relay loop or the UI.
        let app = self.clone();
        std::thread::spawn(move || {
            let mut due = 0u64;
            loop {
                if *app.stop.lock().unwrap() { break; }
                let on = app.rune_settings().is_some_and(|(on, _)| on);
                if !on { due = 0; std::thread::sleep(Duration::from_millis(500)); continue; }
                let now = commons_proto::now_secs();
                if now >= due { due = now + RUNE_POLL_SECS; app.poll_rune(); }
                std::thread::sleep(Duration::from_millis(500));
            }
        });
    }

    /// Test hook: hold the client for `ms` from another thread, the way a long action or a sync round does, so a test can
    /// see what a read answers meanwhile.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn debug_hold(self: &Arc<Self>, ms: u64) {
        let app = self.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || { let g = app.lock(); tx.send(()).unwrap(); std::thread::sleep(Duration::from_millis(ms)); drop(g); });
        rx.recv().unwrap();   // held before this returns
    }

    /// What the poller needs, without keeping the client while rune is asked.
    fn rune_settings(&self) -> Option<(bool, String)> {
        let g = self.lock();
        let c = g.as_ref()?;
        Some((c.st.share_listening, c.rune_url()))
    }

    /// One look at rune. A failure leaves the last known status alone: rune may just be busy, and the receivers'
    /// TTL ends a status whose sender has really gone quiet.
    fn poll_rune(&self) {
        let Some((on, url)) = self.rune_settings() else { return };
        if !on { return; }
        let got = crate::rune::now_playing(&url);
        let mut g = self.lock();
        let Some(c) = g.as_mut() else { return };
        if !c.st.share_listening { return; } // turned off while we were asking
        match got {
            Ok(now) => { let _ = c.update_listening(now); }
            Err(e) => log::info!("rune at {url}: {e}"),
        }
    }

    pub fn call(&self, method: &str, a: Value) -> R {
        if method.starts_with("notify_") { return self.notify.call(method, &a); }
        let in_call = matches!(method, "call_state" | "call_token" | "game_mute_wait");
        if in_call { self.saw_call(); }
        let r = self.dispatch(method, a);
        if in_call && r.is_ok() { self.saw_call(); }
        if !matches!(method, "snapshot" | "messages" | "state" | "file_read" | "file_transfers" | "avatar_versions" | "avatar_data" | "pairing_status" | "game_mute_wait" | "call_presence" | "live_poll" | "ring_poll" | "wall_get" | "notify_get" | "notify_beat" | "pcaudio_status" | "pcaudio_caps" | "pcaudio_streams") { self.nudge(); }
        r
    }

    #[track_caller]
    fn with<T>(&self, f: impl FnOnce(&mut Client) -> Result<T, String>) -> Result<T, String> {
        let mut g = self.lock_at(std::panic::Location::caller());
        match g.as_mut() { Some(c) => f(c), None => Err("locked".into()) }
    }

    /// A read-only call. If the client is busy with something long (an admin action waiting for the relay), the last
    /// answer is good enough: the UI must not freeze for that. But only an answer that is still current, made since the
    /// client was last held for a change. An older one would show the UI a state it has already moved past: the member
    /// list from before the person was just made an admin, a friend's "listening to" from before the status that ended
    /// it arrived. For those the client is waited for, within reason; what usually holds it (a sync round, the live
    /// channel's threads) is done in well under a second, and a long action keeps the old answer coming meanwhile.
    #[track_caller]
    fn read(&self, key: String, f: impl FnOnce(&Client) -> Value) -> R {
        let who = std::panic::Location::caller();
        let try_lock = || match self.client.try_lock() { Ok(g) => Some(g), Err(TryLockError::Poisoned(p)) => Some(p.into_inner()), Err(TryLockError::WouldBlock) => None };
        let t = Instant::now();
        let mut g = try_lock();
        if g.is_none() {
            let cached = self.seen.lock().unwrap().get(&key).cloned();
            if let Some((v, n, at)) = &cached {
                if *n == self.changes.load(Ordering::SeqCst) {
                    if trace_on() { eprintln!("[read] stale {key}: answered from a cache {}ms old", at.elapsed().as_millis()); }
                    return Ok(v.clone());
                }
            }
            while g.is_none() && t.elapsed() < READ_WAIT { std::thread::sleep(Duration::from_millis(2)); g = try_lock(); }
            if trace_on() { eprintln!("[read] after a change {key}: waited {}ms, {}", t.elapsed().as_millis(), if g.is_some() { "fresh" } else if cached.is_some() { "still busy: the old answer" } else { "still busy: waiting on"}); }
            if g.is_none() {
                if let Some((v, _, _)) = cached { return Ok(v); }   // a long action is still running: the old answer rather than a frozen UI
            }
        }
        let g = g.unwrap_or_else(|| self.client.lock().unwrap_or_else(|p| p.into_inner()));
        let g = Held { g, at: Instant::now(), waited: t.elapsed(), who, changes: None };   // a read changes nothing
        let v = f(g.as_ref().ok_or("locked")?);
        let n = self.changes.load(Ordering::SeqCst);   // taken while the client is held: nothing can be changing it in between
        self.seen.lock().unwrap().insert(key, (v.clone(), n, Instant::now()));
        Ok(v)
    }

    /// A poll that does not wait: if the client is busy with something long it says so and the caller asks again a moment later.
    #[track_caller]
    fn try_read(&self, f: impl FnOnce(&Client) -> Value) -> R {
        let g = match self.client.try_lock() {
            Ok(g) => g,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => {
                if trace_on() { let who = std::panic::Location::caller(); eprintln!("[read] busy {}:{}", who.file(), who.line()); }
                return Ok(json!({"busy": true, "events": [], "head": 0, "cells": []}));
            }
        };
        Ok(f(g.as_ref().ok_or("locked")?))
    }

    /// Before anything is created for a relay: is it there, and will it have us?
    fn probe(&self, relay: &str, invite: Option<&str>) -> Result<(), String> {
        let api = Api { t: HttpTransport::new(relay).into_arc() };
        api.health().map_err(|x| unreachable_text(relay, &format!("{x:#}")))?;
        let Some(invite) = invite else { return Ok(()) };
        let Ok(info) = api.info(None) else { return Ok(()) };
        if !info.gated { return Ok(()); }
        if invite.is_empty() { return Err("This relay only takes new accounts with its invite secret. Ask whoever runs it, and enter it under Relay server.".into()); }
        if !api.info(Some(invite)).map_err(|x| format!("{x:#}"))?.ok { return Err("That is not this relay's invite secret.".into()); }
        Ok(())
    }

    /// The sealed invite for this password at this relay: from the short-lived memory if it was fetched a moment ago, else from the relay.
    fn pw_invite_link(&self, api: &Api, relay: &str, pw: &str) -> Result<String, String> {
        use sha2::{Digest, Sha256};
        let key = hex::encode(Sha256::digest(format!("{}|{}", crate::pwinvite::canon_relay(relay), crate::pwinvite::normalize(pw)).as_bytes()));
        if let Some((k, link, at)) = self.pw_link.lock().unwrap().as_ref() {
            if *k == key && at.elapsed() < std::time::Duration::from_secs(600) { return Ok(link.clone()); }
        }
        let link = crate::pwinvite::lookup(api, relay, pw, self.kdf).map_err(e)?;
        *self.pw_link.lock().unwrap() = Some((key, link.clone(), std::time::Instant::now()));
        Ok(link)
    }
    fn forget_pw_link(&self) { *self.pw_link.lock().unwrap() = None; }

    fn relay_arg(&self, a: &Value) -> Result<String, String> {
        let r = if s(a, "relay").trim().is_empty() { self.default_relay.clone() } else { s(a, "relay") };
        if r.trim().is_empty() { return Err("Paste the invite link you were sent into the first box. No link? Enter the relay's address under Relay server.".into()); }
        clean_relay_url(&r)
    }

    fn open_with(&self, f: impl FnOnce(Arc<dyn crate::transport::Transport>) -> anyhow::Result<Client>, relay: &str) -> R {
        let c = f(HttpTransport::new(relay).into_arc()).map_err(|x| format!("{x:#}"))?;
        *self.lock() = Some(c);
        Ok(json!({"ok": true}))
    }

    /// A decrypted attachment, from memory if it was opened a moment ago.
    fn open_file(&self, gid: &str, id: &str) -> Result<(String, String, Arc<Vec<u8>>), String> {
        let key = format!("{gid}/{id}");
        let mut f = self.with(|c| c.file_locate(gid, id).map_err(e))?;
        let (name, mime) = (f.name.clone(), f.mime.clone());
        if let Some((_, d)) = self.files.lock().unwrap().iter().find(|(k, _)| *k == key) { return Ok((name, mime, d.clone())); }
        let fresh = f.cached.is_none();
        let (api, k) = self.with(|c| c.net_handle().map_err(e))?;
        let plain = f.bytes(&api, &k).map_err(e)?; // the slow part, with the client free
        if fresh { let _ = self.with(|c| { c.file_cache(&f.slot, &plain); Ok(()) }); }
        let d = Arc::new(plain);
        let mut files = self.files.lock().unwrap();
        files.push((key, d.clone()));
        if files.len() > OPEN_FILES { files.remove(0); }
        Ok((name, mime, d))
    }

    fn dispatch(&self, m: &str, a: Value) -> R {
        match m {
            // Sending one app's sound into a call (Linux, PipeWire). Runs local programs with fixed arguments: see pcaudio.rs.
            m if m.starts_with("pcaudio_") => crate::pcaudio::rpc(m, &a),
            "state" => {
                // Told before unlocking, so the UI can leave out what a phone cannot do (game keys, updates).
                let mobile = cfg!(any(target_os = "ios", target_os = "android"));
                let g = self.lock();
                match g.as_ref() {
                    None => Ok(json!({"locked": true, "exists": Client::exists(&self.dir), "default_relay": self.default_relay, "mobile": mobile})),
                    Some(c) => Ok(json!({"locked": false, "has_identity": c.has_identity(), "relay": c.st.relay, "mobile": mobile})),
                }
            }
            // What a pasted invite says, before anything is created. Never needs an account.
            "parse_link" => {
                let link = s(&a, "link");
                if let Ok(l) = InviteLink::decode(&link) { return Ok(json!({"kind": "community", "relay": l.relay, "secret": l.secret, "name": l.name, "link": l.encode()})); }
                if let Ok(l) = RelayLink::decode(&link) { return Ok(json!({"kind": "relay", "relay": clean_relay_url(&l.relay)?, "secret": l.secret})); }
                Err("That is not a Megatron invite. It should start with https:// or commons://, and the whole link has to be pasted.".into())
            }
            "create_account" => {
                let relay = self.relay_arg(&a)?;
                let (pass, name, invite) = (s(&a, "passphrase"), s(&a, "name"), s(&a, "invite"));
                if pass.len() < 8 { return Err("Passphrase must be at least 8 characters.".into()); }
                if name.trim().is_empty() { return Err("Please choose a display name.".into()); }
                check_name("A display name", name.trim())?;
                self.probe(&relay, Some(invite.trim()))?;
                self.open_with(|t| Client::create_with_invite(&self.dir, &pass, &relay, name.trim(), invite.trim(), self.kdf, t), &relay)
            }
            "recover_account" => {
                let relay = self.relay_arg(&a)?;
                let (pass, name, key) = (s(&a, "passphrase"), s(&a, "name"), s(&a, "recovery_key"));
                if pass.len() < 8 { return Err("Passphrase must be at least 8 characters.".into()); }
                if name.trim().is_empty() { return Err("Please choose a display name.".into()); }
                check_name("A display name", name.trim())?;
                decode_recovery(&key).map_err(e)?;
                self.probe(&relay, None)?;
                self.open_with(|t| Client::create_from_recovery(&self.dir, &pass, &relay, name.trim(), &key, self.kdf, t), &relay)
            }
            "start_pairing" => {
                let relay = self.relay_arg(&a)?;
                let pass = s(&a, "passphrase");
                if pass.len() < 8 { return Err("Passphrase must be at least 8 characters.".into()); }
                self.probe(&relay, None)?;
                let mut out = String::new();
                let mut chk = String::new();
                self.open_with(|t| { let (c, code) = Client::create_for_pairing(&self.dir, &pass, &relay, "", self.kdf, t)?; out = code; chk = c.pairing_check_new().unwrap_or_default(); Ok(c) }, &relay)?;
                Ok(json!({"code": out, "qr": Client::qr_svg(&out), "check": chk}))
            }
            // The background rounds do the polling; this only reports.
            "pairing_status" => self.with(|c| Ok(json!({"done": c.has_identity()}))),
            // After a restart mid-pairing the code has to come back on screen, or the device is stuck.
            "pairing_resume" => self.with(|c| {
                let code = c.pairing_code().ok_or("no pairing in progress")?;
                Ok(json!({"qr": Client::qr_svg(&code), "check": c.pairing_check_new().unwrap_or_default(), "code": code}))
            }),
            "pairing_abandon" => {
                let c = self.lock().take().ok_or("locked")?;
                c.abandon_pairing().map_err(e)?;
                Ok(json!({"ok": true}))
            }
            "unlock" => {
                if self.lock().is_some() { return Ok(json!({"ok": true})); } // already open in this window
                let pass = s(&a, "passphrase");
                let c = Client::open_with(&self.dir, &pass, |relay| HttpTransport::new(relay).into_arc())
                    .map_err(|x| if format!("{x}").contains("wrong passphrase") { "Wrong passphrase.".to_string() } else { format!("{x:#}") })?;
                *self.lock() = Some(c);
                Ok(json!({"ok": true}))
            }
            "lock" => {
                *self.lock() = None;
                self.seen.lock().unwrap().clear();
                self.windows.lock().unwrap().clear();
                self.files.lock().unwrap().clear();
                Ok(json!({"ok": true}))
            }
            "snapshot" => same_or(self.read("snapshot".into(), snapshot), &a),
            "messages" => {
                let (gid, around) = (s(&a, "gid"), s(&a, "around"));
                let limit = a.get("limit").and_then(|x| x.as_u64()).unwrap_or(200).clamp(1, 5000) as usize;
                let key = format!("messages/{gid}/{limit}/{around}");
                let v = self.read(key.clone(), |c| messages(c, &gid, limit, &around))?;
                self.messages_answer(key, v, &a)
            }
            "mark_read" => self.with(|c| { c.mark_read(&s(&a, "gid")); c.save().map_err(e)?; Ok(json!({})) }),
            "send_text" => self.with(|c| { let r = a.get("reply").and_then(|x| x.as_str()).map(|x| x.to_string()); c.send_text(&s(&a, "gid"), &s(&a, "text"), r).map(|id| json!({"id": id})).map_err(e) }),
            "edit" => self.with(|c| c.edit_message(&s(&a, "gid"), &s(&a, "id"), &s(&a, "text")).map(|_| json!({})).map_err(e)),
            "delete" => self.with(|c| c.delete_message(&s(&a, "gid"), &s(&a, "id")).map(|_| json!({})).map_err(e)),
            "react" => self.with(|c| c.react(&s(&a, "gid"), &s(&a, "id"), &s(&a, "emoji"), a["on"].as_bool().unwrap_or(true)).map(|_| json!({})).map_err(e)),
            "typing" => self.with(|c| { c.send_typing(&s(&a, "gid")); Ok(json!({})) }),
            "live_send" => self.with(|c| c.live_send(&s(&a, "gid"), &s(&a, "kind"), &s(&a, "data")).map(|sent| json!({"sent": sent})).map_err(e)),
            "ring_send" => self.with(|c| { let to = s(&a, "to"); c.ring_send(&s(&a, "gid"), (!to.is_empty()).then_some(to.as_str())).map(|id| json!({"id": id})).map_err(e) }),
            "ring_end" => self.with(|c| { let to = s(&a, "to"); c.ring_end(&s(&a, "gid"), &s(&a, "id"), (!to.is_empty()).then_some(to.as_str())).map(|_| json!({})).map_err(e) }),
            // Test bridge only: a ring with a time of the caller's choosing, to show a stale one rings nobody.
            #[cfg(feature = "devbridge")]
            "debug_ring" => self.with(|c| c.debug_ring_aged(&s(&a, "gid"), a["age"].as_u64().unwrap_or(0)).map(|id| json!({"id": id})).map_err(e)),
            // One-to-one calls (direct.rs): call a person, and stop (or allow again) calls from one.
            "direct_call" => self.with(|c| c.direct_call(&s(&a, "peer")).map(|d| json!({"cid": d.cid, "room": d.room, "peer": d.peer, "invited": d.invited})).map_err(e)),
            #[cfg(feature = "devbridge")]
            "debug_direct_anyway" => self.with(|c| c.debug_direct_call_anyway(&s(&a, "peer")).map(|d| json!({"cid": d.cid, "room": d.room, "invited": d.invited})).map_err(e)),
            "direct_refuse" => self.with(|c| c.direct_refuse(&s(&a, "account"), a["on"].as_bool().unwrap_or(true)).map(|_| json!({})).map_err(e)),
            "ring_poll" => {
                let since = a["since"].as_u64().unwrap_or(0);
                self.try_read(|c| json!({"events": c.ring_poll(since).into_iter().map(|v| json!({"id": v.id, "gid": v.gid, "account": v.account, "ring": v.ring, "ts": v.ts, "end": v.end, "direct": v.direct})).collect::<Vec<_>>(), "head": c.ring_next}))
            }
            // Whether live events now travel on the unstored channel (relay known to carry it, not the log fallback). Presence on the cards sends only then.
            "live_chan" => self.try_read(|c| json!({"chan": c.live_chan_on() && c.live_support() == crate::live::Support::Yes})),
            // A poll: it never waits behind a long action (it answers "busy" and is asked again) and never wakes the sync loop.
            "live_poll" => {
                let (gid, since) = (s(&a, "gid"), a["since"].as_u64().unwrap_or(0));
                self.try_read(|c| {
                    let evs = c.live_poll(since, (!gid.is_empty()).then_some(gid.as_str()));
                    json!({"events": evs.into_iter().map(|v| json!({"id": v.id, "gid": v.gid, "account": v.account, "kind": v.kind, "data": v.data, "ts": v.ts})).collect::<Vec<_>>(), "head": c.live_next})
                })
            }
            // Owner only. `room` empty = the community's own card; `main` empty = back to the default colours.
            "set_style" => self.with(|c| {
                let (room, main, hi) = (s(&a, "room"), s(&a, "main"), s(&a, "hi"));
                let duo = if main.is_empty() && hi.is_empty() { None } else { Some(crate::envelope::Duo { main, hi }) };
                c.set_style(&s(&a, "cid"), (!room.is_empty()).then_some(room.as_str()), duo).map(|_| json!({})).map_err(e)
            }),
            // Owner only. `room` empty = the community's own card. `card` / `chat` (0..=31) and `dim` (0..=200) are numbers, or null / missing for automatic;
            // all three missing puts the look back to automatic.
            "set_look" => self.with(|c| {
                let room = s(&a, "room");
                let n = |k: &str| a[k].as_u64();
                let (card, chat, dim) = (n("card"), n("chat"), n("dim"));
                if [card, chat].iter().flatten().any(|v| *v > 255) || dim.is_some_and(|v| v > 65535) { return Err("a look has patterns 0 to 31 and a strength of 0 to 200 percent".to_string()); }
                // `oracle_quiet` is kept as it is unless the call names it, so a UI that only knows patterns never switches Oracle back on.
                let cid = s(&a, "cid");
                let room_opt = (!room.is_empty()).then_some(room.as_str());
                let quiet = a["oracle_quiet"].as_bool().or_else(|| c.look_of(&cid, room_opt).and_then(|l| l.oracle_quiet));
                let draw = a["oracle_draw"].as_bool().or_else(|| c.look_of(&cid, room_opt).and_then(|l| l.oracle_draw));
                let look = crate::envelope::Look { card: card.map(|v| v as u8), chat: chat.map(|v| v as u8), dim: dim.map(|v| v as u16), oracle_quiet: quiet, oracle_draw: draw };
                c.set_look(&cid, room_opt, Some(look)).map(|_| json!({})).map_err(e)
            }),
            // Owner only: `on` true lets the Oracle bot doodle on this room's backdrop now and then; false stops it.
            "set_oracle_draw" => self.with(|c| {
                let Some(on) = a["on"].as_bool() else { return Err("on must be true or false".to_string()) };
                c.set_oracle_draw(&s(&a, "cid"), &s(&a, "room"), on).map(|_| json!({})).map_err(e)
            }),
            // Owner only: `on` true keeps the Oracle bot out of this room unless someone @mentions it; false lets it back in.
            "set_oracle_quiet" => self.with(|c| {
                let Some(on) = a["on"].as_bool() else { return Err("on must be true or false".to_string()) };
                c.set_oracle_quiet(&s(&a, "cid"), &s(&a, "room"), on).map(|_| json!({})).map_err(e)
            }),
            // Owner only: `lock` is "1" / "0" / "" (leave), `clear` is true to start the wall over for everyone.
            "set_wall" => self.with(|c| {
                let lock = match s(&a, "lock").as_str() { "1" => Some(true), "0" => Some(false), _ => None };
                c.set_wall(&s(&a, "cid"), &s(&a, "room"), lock, a["clear"].as_bool().unwrap_or(false)).map(|_| json!({})).map_err(e)
            }),
            "wall_draw" => self.with(|c| {
                let cells: Vec<(u32, u32, u8, u8)> = a["cells"].as_array().map(|v| v.iter().filter_map(|x| { let x = x.as_array()?; Some((x.first()?.as_u64()? as u32, x.get(1)?.as_u64()? as u32, x.get(2)?.as_u64()? as u8, x.get(3)?.as_u64()? as u8)) }).collect()).unwrap_or_default();
                c.wall_draw_to(&s(&a, "gid"), &cells, a["share"].as_bool().unwrap_or(true)).map(|sent| json!({"sent": sent})).map_err(e)
            }),
            // The wall as it stands. With `rev` equal to the last answer's, only the small header comes back.
            "wall_get" => {
                let (gid, rev) = (s(&a, "gid"), a["rev"].as_u64());
                self.try_read(|c| {
                    let v = c.wall_view(&gid);
                    let same = rev == Some(v.rev);
                    json!({"carrier": gid.strip_prefix(crate::wall::CWALL_PREFIX).and_then(|cid| c.wall_carrier(cid)), "gen": v.gen, "locked": v.locked, "rev": v.rev, "snap_at": v.snap_at, "peer_at": v.peer_at, "same": same,
                        "cells": if same { vec![] } else { v.cells.iter().map(|(p, g, col, by)| json!([p, g, col, by])).collect::<Vec<_>>() }})
                })
            }
            "wall_snapshot" => self.with(|c| c.wall_snapshot(&s(&a, "gid")).map(|chunks| json!({"chunks": chunks})).map_err(e)),
            "set_listening_sharing" => self.with(|c| c.set_listening_sharing(a["on"].as_bool().unwrap_or(false), &s(&a, "rune")).map(|_| json!({})).map_err(e)),
            "send_file" => {
                let data = unb64(&s(&a, "data")).map_err(e)?;
                let (f, (api, k)) = self.with(|c| Ok((c.file_prepare(&s(&a, "gid"), &s(&a, "name"), &s(&a, "mime"), &data, &s(&a, "caption")).map_err(e)?, c.net_handle().map_err(e)?)))?;
                f.upload(&api, &k).map_err(e)?; // the slow part, with the client free
                self.with(|c| c.file_queue(f).map(|id| json!({"id": id})).map_err(e))
            }
            "fetch_file" => {
                let (n, m, d) = self.open_file(&s(&a, "gid"), &s(&a, "id"))?;
                Ok(json!({"name": n, "save_name": safe_file_name(&n), "mime": m, "data": b64(&d)}))
            }
            // A profile picture, shrunk on the device by the UI: encrypted here, uploaded, then announced in the
            // account's profile message (one picture per account, for every community).
            "set_avatar" => {
                let data = unb64(&s(&a, "data")).map_err(e)?;
                let (f, (api, k)) = self.with(|c| Ok((c.avatar_prepare(&s(&a, "mime"), &s(&a, "shape"), &data).map_err(e)?, c.net_handle().map_err(e)?)))?;
                f.upload(&api, &k).map_err(e)?; // the slow part, with the client free
                let r = f.into_ref();
                let blob = r.blob.clone();
                self.with(|c| c.avatar_set(r).map(|_| json!({"blob": blob})).map_err(e))
            }
            "remove_avatar" => self.with(|c| c.avatar_clear().map(|_| json!({})).map_err(e)),
            // Account -> blob id: the UI downloads only the pictures that changed.
            "avatar_versions" => self.read("avatar_versions".into(), |c| json!(c.avatar_versions())),
            "avatar_data" => {
                let account = s(&a, "account");
                let mut f = self.with(|c| c.avatar_locate(&account).map_err(e))?;
                let fresh = f.cached.is_none();
                let (api, k) = self.with(|c| c.net_handle().map_err(e))?;
                let plain = f.bytes(&api, &k).map_err(e)?; // the slow part, with the client free
                if fresh { let _ = self.with(|c| { c.file_cache(&f.slot, &plain); Ok(()) }); }
                Ok(json!({"blob": f.avatar.blob, "mime": f.avatar.mime, "shape": f.avatar.shape, "size": plain.len(), "data": b64(&plain)}))
            }
            // The same file, without one message of tens of megabytes: open it, then read it in pieces.
            "file_open" => {
                let (n, m, d) = self.open_file(&s(&a, "gid"), &s(&a, "id"))?;
                Ok(json!({"name": n, "save_name": safe_file_name(&n), "mime": m, "size": d.len(), "chunk": FILE_CHUNK}))
            }
            "file_read" => {
                let (_, _, d) = self.open_file(&s(&a, "gid"), &s(&a, "id"))?;
                let at = a.get("offset").and_then(|x| x.as_u64()).unwrap_or(0).min(d.len() as u64) as usize;
                Ok(json!({"data": b64(&d[at..(at + FILE_CHUNK).min(d.len())])}))
            }
            // Large files: the UI hands over one chunk of the File at a time; the core encrypts and uploads it with
            // the client free, and never holds more than that chunk. The relay remembers what arrived, so offering
            // the same file again (after a drop or a restart) continues instead of starting over.
            "file_upload_begin" => {
                let (gid, name, mime, caption) = (s(&a, "gid"), s(&a, "name"), s(&a, "mime"), s(&a, "caption"));
                let size = a.get("size").and_then(|x| x.as_u64()).ok_or("missing file size")?;
                let lm = a.get("last_modified").and_then(|x| x.as_u64()).unwrap_or(0);
                let (u, (api, k)) = self.with(|c| Ok((c.large_upload_begin(&gid, &name, &mime, size, lm, &caption).map_err(e)?, c.net_handle().map_err(e)?)))?;
                let resp = api.large_begin(&k, &u.id, &LargeBeginReq { gid: gid.clone(), size, chunk_size: u.chunk_size }).map_err(xe)?;
                let u = self.with(|c| c.large_upload_started(&u.id, &resp).map_err(e))?;
                Ok(json!({"id": u.id, "chunk_size": u.chunk_size, "chunks": u.chunks, "uploaded": u.uploaded, "expires": u.expires, "complete": u.complete, "name": u.name,
                          "need_read": u.need_read, "verify": u.verify, "hashed": u.hashed}))
            }
            // Re-sync with the relay for a session that already exists (after a drop or a relay restart), without the file.
            "file_upload_sync" => {
                let id = s(&a, "id");
                let (gid, size, cs, (api, k)) = self.with(|c| {
                    let u = c.st.large_uploads.get(&id).ok_or("no such upload")?;
                    Ok((u.gid.clone(), u.size, u.chunk_size, c.net_handle().map_err(e)?))
                })?;
                let resp = api.large_begin(&k, &id, &LargeBeginReq { gid, size, chunk_size: cs }).map_err(xe)?;
                let u = self.with(|c| c.large_upload_started(&id, &resp).map_err(e))?;
                Ok(json!({"id": u.id, "chunks": u.chunks, "uploaded": u.uploaded, "expires": u.expires, "complete": u.complete, "need_read": u.need_read, "verify": u.verify, "hashed": u.hashed}))
            }
            "file_upload_check" => {
                let (id, index) = (s(&a, "id"), a.get("index").and_then(|x| x.as_u64()).ok_or("missing chunk index")?);
                let plain = unb64(&s(&a, "data")).map_err(e)?;
                self.with(|c| c.large_chunk_check(&id, index, &plain).map(|_| json!({})).map_err(e))
            }
            "file_upload_chunk" => {
                let id = s(&a, "id");
                let index = a.get("index").and_then(|x| x.as_u64()).ok_or("missing chunk index")?;
                let plain = unb64(&s(&a, "data")).map_err(e)?;
                let prep = self.with(|c| c.large_chunk_prepare(&id, index, &plain).map_err(e))?;
                if prep.needed {
                    let (api, k, blob) = self.with(|c| {
                        let blob = c.st.large_uploads.get(&id).map(|u| u.blob.clone()).ok_or("no such upload")?;
                        let (api, k) = c.net_handle().map_err(e)?;
                        Ok((api, k, blob))
                    })?;
                    api.large_put_chunk(&k, &blob, index, &prep.ct).map_err(xe)?; // slow part, client free
                }
                self.with(|c| { c.large_chunk_done(&id, index); Ok(json!({"sent": prep.needed})) })
            }
            // Needs only the saved hashes, never the file; asking again after it already went through is harmless.
            "file_upload_finish" => {
                let id = s(&a, "id");
                if let Some(mid) = self.with(|c| Ok(c.large_sent_message(&id)))? { return Ok(json!({"id": mid})); }
                let (blob, root, (api, k)) = self.with(|c| {
                    let (b, r) = c.large_finish_prepare(&id).map_err(e)?;
                    Ok((b, r, c.net_handle().map_err(e)?))
                })?;
                api.large_finish(&k, &blob, &root).map_err(xe)?;
                self.with(|c| c.large_finish_done(&id).map(|mid| json!({"id": mid})).map_err(e))
            }
            "file_upload_pause" => {
                let (id, paused) = (s(&a, "id"), a.get("paused").and_then(|x| x.as_bool()).unwrap_or(true));
                self.with(|c| c.large_upload_pause(&id, paused).map(|_| json!({})).map_err(e))
            }
            "file_download_pause" => {
                let (id, paused) = (s(&a, "id"), a.get("paused").and_then(|x| x.as_bool()).unwrap_or(true));
                self.with(|c| c.large_download_pause(&id, paused).map(|_| json!({})).map_err(e))
            }
            // Everything unfinished, so it can be found again after a reload: uploads waiting for their file, downloads
            // partly saved.
            "file_transfers" => self.with(|c| {
                let ups: Vec<Value> = c.st.large_uploads.values().map(|u| json!({
                    "id": u.blob, "gid": u.gid, "name": u.name, "size": u.size, "last_modified": u.last_modified, "chunks": u.chunks,
                    "hashed": u.hashes.len(), "uploaded": u.uploaded.len(), "expires": u.expires, "paused": u.paused,
                    "ready": u.hashes.len() as u64 == u.chunks, "chunk_size": u.chunk_size })).collect();
                let downs: Vec<Value> = c.st.large_downloads.values().filter(|d| !d.finished).map(|d| json!({
                    "id": d.msg_id, "gid": d.gid, "name": d.name, "size": d.size, "chunks": d.chunks, "chunk_size": d.chunk_size, "done": d.done,
                    "expires": d.expires, "paused": d.paused, "dir": d.dir })).collect();
                Ok(json!({"uploads": ups, "downloads": downs, "now": commons_proto::now_secs()}))
            }),
            "file_upload_cancel" => {
                let id = s(&a, "id");
                let blob = self.with(|c| c.large_upload_cancel(&id).map_err(e))?;
                if let Some(b) = blob {
                    // Best effort: a relay that cannot be reached now will remove the orphan when it expires.
                    if let Ok((api, k)) = self.with(|c| c.net_handle().map_err(e)) { let _ = api.large_delete(&k, &b); }
                }
                Ok(json!({}))
            }
            "file_download_begin" => {
                let folder = s(&a, "folder");
                let d = self.with(|c| c.large_download_begin_in(&s(&a, "gid"), &s(&a, "id"), Some(&folder)).map_err(e))?;
                Ok(json!({"done": d.done, "chunks": d.chunks, "size": d.size, "chunk_size": d.chunk_size, "save_name": d.save_name, "finished": d.finished, "path": d.path, "expires": d.expires, "name": d.name}))
            }
            "file_download_chunk" => {
                let (gid, id) = (s(&a, "gid"), s(&a, "id"));
                let st = self.with(|c| c.large_download_state(&id).ok_or_else(|| "no download in progress".to_string()))?;
                if st.finished { return Ok(json!({"done": st.done, "chunks": st.chunks, "size": st.size, "finished": true, "path": st.path, "save_name": st.save_name})); }
                let (api, k) = self.with(|c| c.net_handle().map_err(e))?;
                let ct = api.large_get_chunk(&k, &st.blob, st.done).map_err(xe)?; // slow part, client free
                let step = self.with(|c| c.large_download_write(&gid, &id, &ct).map_err(e))?;
                Ok(json!({"done": step.done, "chunks": step.chunks, "size": step.size, "finished": step.finished, "path": step.path, "save_name": st.save_name}))
            }
            "file_download_cancel" => self.with(|c| c.large_download_cancel(&s(&a, "id")).map(|_| json!({})).map_err(e)),
            "file_show_in_folder" => self.with(|c| c.show_download_in_folder(&s(&a, "id")).map(|opened| json!({"opened": opened})).map_err(e)),
            // Only the sender's account may do this, and only after the relay really let go of the bytes.
            "file_delete" => {
                let (gid, id) = (s(&a, "gid"), s(&a, "id"));
                let blob = self.with(|c| c.large_delete_prepare(&gid, &id).map_err(e))?;
                let (api, k) = self.with(|c| c.net_handle().map_err(e))?;
                match api.large_delete(&k, &blob) {
                    Ok(()) => {}
                    Err(err) if crate::transport::relay_status(&err) == Some(404) => {}
                    Err(err) => return Err(e(err)),
                }
                self.with(|c| c.large_delete_done(&gid, &id).map(|_| json!({})).map_err(e))
            }
            "pick_folder" => crate::large::pick_folder(&s(&a, "start")).map(|path| json!({"path": path})).map_err(e),
            "set_download_dir" => self.with(|c| c.set_download_dir(&s(&a, "dir")).map(|dir| json!({"dir": dir})).map_err(e)),
            "create_community" => self.with(|c| c.create_community(&s(&a, "name")).map(|id| json!({"id": id})).map_err(e)),
            "create_channel" => self.with(|c| {
                // `kind` is optional: an app from before voice rooms sends none, and "" is a text room.
                let kind = a.get("kind").and_then(|x| x.as_str()).unwrap_or("");
                c.create_channel_kind(&s(&a, "cid"), &s(&a, "name"), kind).map(|id| json!({"id": id})).map_err(e)
            }),
            "create_invite" => {
                let uses = a.get("uses").and_then(|x| x.as_u64()).map(|x| x as u32);
                let ttl = a.get("ttl").and_then(|x| x.as_u64()).unwrap_or(7 * 86400);
                let pw = s(&a, "password");
                if !pw.trim().is_empty() {
                    // The slow key derivation runs before the client is locked, so the app stays responsive.
                    crate::pwinvite::check(&pw).map_err(e)?;
                    let (relay, cid) = (self.with(|c| Ok(c.st.relay.clone()))?, s(&a, "cid"));
                    let d = crate::pwinvite::derive(&pw, &relay, self.kdf).map_err(e)?;
                    return self.with(|c| {
                        c.prune_pw_invites();
                        let made = c.create_invite_password(&cid, &d, uses, ttl).map_err(e)?;
                        Ok(json!({"password": true, "id": made.invite, "expires": made.expires, "download": format!("{}/join", relay.trim_end_matches('/')), "relay": relay,
                            "link": made.link, "signup_needs_secret": made.signup_needs_secret}))
                    });
                }
                self.with(|c| {
                    let made = c.create_invite_checked(&s(&a, "cid"), uses, ttl).map_err(e)?;
                    let web = InviteLink::decode(&made.link).ok().and_then(|l| l.web());
                    Ok(json!({"link": web.as_deref().unwrap_or(&made.link), "qr": Client::qr_svg(web.as_deref().unwrap_or(&made.link)), "signup_needs_secret": made.signup_needs_secret}))
                })
            }
            // How good a password for an invite is, and a fresh suggestion. Needs no account.
            "recovery_saved" => self.with(|c| c.set_recovery_unsaved(a.get("unsaved").and_then(|x| x.as_bool()).unwrap_or(false)).map(|_| json!({})).map_err(e)),
            "password_strength" => { let st = crate::pwinvite::strength(&s(&a, "password")); Ok(json!({"bits": st.bits, "ok": st.ok, "warn": st.warn})) }
            "suggest_password" => Ok(json!({"password": crate::pwinvite::suggest()})),
            // A new account that joins with only the inviter's password. Nothing is made unless the password finds an invite.
            "create_account_password" => {
                let relay = self.relay_arg(&a)?;
                let (pass, name, pw) = (s(&a, "passphrase"), s(&a, "name"), s(&a, "password"));
                if pass.len() < 8 { return Err("The password for this device must be at least 8 characters.".into()); }
                if name.trim().is_empty() { return Err("Please choose a name.".into()); }
                check_name("A display name", name.trim())?;
                // Everything that can be checked without the invite comes first: a fetch spends one of the invite's few.
                if Client::exists(&self.dir) { return Err("This computer already has an account set up. Unlock it, or remove it first.".into()); }
                let api = Api { t: HttpTransport::new(&relay).into_arc() };
                api.health().map_err(|x| unreachable_text(&relay, &format!("{x:#}")))?;
                let link = self.pw_invite_link(&api, &relay, &pw)?;
                let l = InviteLink::decode(&link).map_err(|_| crate::pwinvite::NOT_FOUND.to_string())?;
                if !crate::pwinvite::same_relay(&l.relay, &relay) { return Err(crate::pwinvite::NOT_FOUND.into()); }
                self.probe(&relay, Some(l.secret.trim()))?;
                self.open_with(|t| Client::create_with_invite(&self.dir, &pass, &relay, name.trim(), l.secret.trim(), self.kdf, t), &relay)?;
                self.forget_pw_link();
                self.with(|c| c.set_recovery_unsaved(true).map_err(e))?;
                match self.with(|c| c.join_with_link(&link).map_err(e)) {
                    Ok(id) => Ok(json!({"ok": true, "community": id, "name": l.name})),
                    Err(m) => Ok(json!({"ok": true, "join_error": m})),
                }
            }
            // An existing account joins another community with a password.
            "join_password" => {
                let (pw, relay) = (s(&a, "password"), self.with(|c| Ok(c.st.relay.clone()))?);
                let api = Api { t: HttpTransport::new(&relay).into_arc() };
                let link = self.pw_invite_link(&api, &relay, &pw)?;
                let out = self.with(|c| c.join_with_link(&link).map(|id| json!({"id": id})).map_err(e));
                if out.is_ok() { self.forget_pw_link(); }
                out
            }
            "join" => self.with(|c| c.join_with_link(&s(&a, "link")).map(|id| json!({"id": id})).map_err(e)),
            "cancel_join" => self.with(|c| c.cancel_join(&s(&a, "id")).map(|_| json!({})).map_err(e)),
            "kick" => self.with(|c| c.kick(&s(&a, "cid"), &s(&a, "account")).map(|_| json!({})).map_err(e)),
            "set_role" => self.with(|c| c.set_role(&s(&a, "cid"), &s(&a, "account"), if s(&a, "role") == "admin" { Role::Admin } else { Role::Member }).map(|_| json!({})).map_err(e)),
            "transfer_ownership" => self.with(|c| c.transfer_ownership(&s(&a, "cid"), &s(&a, "account")).map(|_| json!({})).map_err(e)),
            "leave" => self.with(|c| c.leave(&s(&a, "cid")).map(|_| json!({})).map_err(e)),
            "rename_community" => self.with(|c| c.rename_community(&s(&a, "cid"), &s(&a, "name")).map(|_| json!({})).map_err(e)),
            "rename_channel" => self.with(|c| c.rename_channel(&s(&a, "cid"), &s(&a, "room"), &s(&a, "name")).map(|_| json!({})).map_err(e)),
            "delete_channel" => self.with(|c| c.delete_channel(&s(&a, "cid"), &s(&a, "room")).map(|_| json!({})).map_err(e)),
            "unban" => self.with(|c| c.unban(&s(&a, "cid"), &s(&a, "account")).map(|_| json!({})).map_err(e)),
            "revoke_invite" => self.with(|c| c.revoke_invite(&s(&a, "cid"), &s(&a, "id")).map(|_| json!({})).map_err(e)),
            "members" => { let cid = s(&a, "cid"); self.read(format!("members/{cid}"), |c| json!(c.members(&cid).into_iter().map(|m| json!({"account": m.account, "name": m.name, "role": m.role.name(), "devices": m.devices, "verified": m.verified, "me": m.is_me, "tag": m.tag, "same_name": m.same_name, "listening": m.listening.map(|l| json!({"title": l.title, "artist": l.artist}))})).collect::<Vec<_>>())) }
            "safety" => self.with(|c| {
                let acct = s(&a, "account");
                let num = safety_number(&c.my_account(), &acct);
                let payload = format!("commons-verify:{}", num.replace(' ', ""));
                Ok(json!({"number": num, "qr": Client::qr_svg(&payload), "verified": c.st.verified.contains(&acct), "name": c.display_name(&acct)}))
            }),
            "set_verified" => self.with(|c| c.set_verified(&s(&a, "account"), a["on"].as_bool().unwrap_or(true)).map(|_| json!({})).map_err(e)),
            "devices" => {
                let (me, acct, (api, k)) = self.with(|c| Ok((c.my_device(), c.my_account(), c.net_handle().map_err(e)?)))?;
                let got = api.devices(&k, &acct).map_err(e)?;
                let list = self.with(|c| Ok(c.merge_directory(&acct, got)))?;
                Ok(json!(list.into_iter().map(|(d, rev)| json!({"device": d.device, "created": d.created, "revoked": rev, "this": d.device == me})).collect::<Vec<_>>()))
            }
            "revoke_device" => {
                let dev = s(&a, "device");
                let (acct, sig, api) = self.with(|c| Ok((c.my_account(), c.revocation_sig(&dev).map_err(e)?, c.net_handle().map_err(e)?.0)))?;
                api.revoke(&acct, &dev, &sig).map_err(e)?;
                self.with(|c| c.revocation_done(&dev, sig).map(|_| json!({})).map_err(e))
            }
            // Shows the check digits before anything is sent: comparing them afterwards would be too late.
            "pairing_preview" => self.with(|c| c.pairing_check_code(&s(&a, "code")).map(|chk| json!({"check": chk})).map_err(e)),
            "approve_pairing" => {
                let (rid, sealed, chk, (api, k)) = self.with(|c| { let (r, x, chk) = c.pairing_seal(&s(&a, "code")).map_err(e)?; Ok((r, x, chk, c.net_handle().map_err(e)?)) })?;
                api.rv_put(&k, &rid, "a", &sealed).map_err(e)?;
                self.with(|c| { c.notice("pair", "pairing data sent; the new device will appear in your device list once it registers"); Ok(json!({"check": chk})) })
            }
            "recovery_key" => self.with(|c| c.recovery_key_with_passphrase(&s(&a, "passphrase")).map(|k| json!({"key": k})).map_err(e)),
            "search" => self.with(|c| {
                let r = c.search(&s(&a, "q"));
                Ok(json!(r.into_iter().map(|(cid, gid, m)| {
                    let cname = c.st.communities.get(&cid).map(|x| x.name.clone()).unwrap_or_default();
                    let chname = c.st.communities.get(&cid).and_then(|x| x.channels.iter().find(|ch| ch.id == gid)).map(|ch| ch.name.clone()).unwrap_or_default();
                    let text = match &m.body { MsgBody::Text(t) => t.clone(), MsgBody::File { name, .. } => format!("[file] {name}"), MsgBody::Notice(t) => member_line(c, t).unwrap_or_else(|| t.clone()) };
                    json!({"cid": cid, "gid": gid, "id": m.id, "community": cname, "channel": chname, "from": c.display_name(&m.sender), "ts": m.ts, "text": text})
                }).collect::<Vec<_>>()))
            }),
            "set_name" => self.with(|c| c.set_name(&s(&a, "name")).map(|_| json!({})).map_err(e)),
            // For a relay that moved, keeping its data: see Client::set_relay.
            "set_relay" => {
                let relay = clean_relay_url(&s(&a, "relay"))?;
                self.probe(&relay, None)?;
                self.with(|c| c.set_relay(&relay, HttpTransport::new(&relay).into_arc()).map(|_| json!({"relay": relay})).map_err(e))
            }
            // The sign-up secret, typed in once on a device that never got it. Checked with the relay before saving.
            "set_relay_invite" => self.with(|c| c.set_relay_invite(&s(&a, "secret")).map(|_| json!({})).map_err(e)),
            "set_typing_enabled" => self.with(|c| { c.st.typing_enabled = a["on"].as_bool().unwrap_or(false); c.save().map_err(e)?; Ok(json!({})) }),
            "notices" => self.read("notices".into(), |c| json!(c.st.notices.iter().rev().take(80).map(|n| json!({"ts": n.ts, "kind": n.kind, "text": n.text})).collect::<Vec<_>>())),
            "call_state" => self.with(|c| {
                let st = c.call_state(&s(&a, "gid")).map_err(e)?;
                let keys: serde_json::Map<String, Value> = st.keys.iter().map(|(d, k)| (d.clone(), json!(b64(k)))).collect();
                Ok(json!({"epoch": st.epoch, "me": st.my_device, "keys": keys,
                          "members": st.members.iter().map(|m| json!({"device": m.device, "account": m.account, "name": m.name, "me": m.is_me})).collect::<Vec<_>>()}))
            }),
            #[cfg(not(any(target_os = "ios", target_os = "android")))]
            "game_keys_get" => {
                let settings = self.with(|c| Ok(c.st.game_keys.clone()))?;
                let i = commons_gamekeys::backend::info(&settings);
                Ok(json!({"settings": serde_json::to_value(&settings).map_err(e)?, "backend": i.backend, "hold": i.hold, "toggle": i.toggle, "mouse": i.mouse, "note": i.note, "permission": i.permission, "portal_possible": cfg!(target_os = "linux"), "error": self.game.status().error}))
            }
            #[cfg(not(any(target_os = "ios", target_os = "android")))]
            "game_keys_set" => {
                let settings: commons_gamekeys::Settings = serde_json::from_value(a.clone()).map_err(|_| "Those settings could not be read.".to_string())?;
                settings.check()?;
                self.with(|c| { c.st.game_keys = settings.clone(); c.save().map_err(e) })?;
                self.game.set_settings(settings);
                Ok(json!({}))
            }
            // Names only, for the picker. It goes no further than this device's own settings page.
            #[cfg(not(any(target_os = "ios", target_os = "android")))]
            "running_programs" => Ok(json!(commons_gamekeys::backend::processes())),
            #[cfg(not(any(target_os = "ios", target_os = "android")))]
            "game_keys_allow" => Ok(json!({"allowed": commons_gamekeys::backend::request_permission()})),
            // A call's heartbeat and its news. It waits outside the client lock: this must not hold anything up, or be held up.
            // A phone has no keyboard for a game to read: the call still asks, and is told nothing is muted.
            #[cfg(not(any(target_os = "ios", target_os = "android")))]
            "game_mute_wait" => {
                if let Ok(g) = self.client.try_lock() { self.game.set_settings(g.as_ref().map(|c| c.st.game_keys.clone()).unwrap_or_default()); }
                let since = a.get("since").and_then(|x| x.as_u64()).unwrap_or(0);
                let st = self.game.wait(since, Duration::from_secs(4));
                Ok(json!({"seq": st.seq, "muted": st.muted, "game": st.game, "running": st.running, "error": st.error}))
            }
            #[cfg(any(target_os = "ios", target_os = "android"))]
            "game_mute_wait" => Ok(json!({"seq": 0, "muted": false, "game": null, "running": false, "error": null})),
            #[cfg(not(any(target_os = "ios", target_os = "android")))]
            "game_mute_clear" => { self.game.user_unmute(); Ok(json!({})) }
            #[cfg(any(target_os = "ios", target_os = "android"))]
            "game_mute_clear" => Ok(json!({})),
            #[cfg(not(any(target_os = "ios", target_os = "android")))]
            "open_call_in_browser" => self.open_call_in_browser(&s(&a, "gid"), &s(&a, "name"), a["launch"].as_bool().unwrap_or(true)),
            #[cfg(any(target_os = "ios", target_os = "android"))]
            "open_call_in_browser" => Err("Calls run in this app on your phone; there is no browser to hand them to.".into()),
            // A link someone posted, opened in the system browser when clicked. Only plain http(s) addresses.
            "open_link" => { let u = s(&a, "url"); if !link_ok(&u) { return Err("that is not a web address this app opens".into()); } Ok(json!({"opened": crate::callbridge::open_in_browser(&u)})) }
            // the relay round trip happens with the client free, like file downloads
            "call_token" => {
                let gid = s(&a, "gid");
                self.with(|c| c.call_state(&gid).map(|_| ()).map_err(e))?;
                let (api, k) = self.with(|c| c.net_handle().map_err(e))?;
                api.call_token(&k, &gid).map(|t| json!({"url": t.url, "token": t.token, "room": t.room, "identity": t.identity, "expires": t.expires, "turn_port": t.turn_port})).map_err(e)
            }
            // Who is in each room's call. Ask the relay, which asks LiveKit (the authoritative list); then map the
            // device ids it returns to names from this device's own group state. Only this community's joined
            // rooms are asked about, and all in one relay request, so a refresh is not one request per room.
            "call_presence" => {
                // one community ("cid"), or several at once ("cids", for the home cards): only voice rooms can hold a call, and the
                // relay takes at most 64 rooms in one question, so ask about the voice rooms first and stop at that limit
                let mut cids: Vec<String> = a.get("cids").and_then(|x| x.as_array()).map(|v| v.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
                if cids.is_empty() { cids.push(s(&a, "cid")); }
                let (api, k, gids) = self.with(|c| {
                    let mut gids: Vec<String> = vec![];
                    for cid in &cids { gids.extend(c.channels_of(cid).into_iter().filter(|ch| ch.joined && ch.kind == "voice").map(|ch| ch.gid)); }
                    // Older apps can still hold a call in a text room, so those rooms are asked about too, after every voice room.
                    for cid in &cids { gids.extend(c.channels_of(cid).into_iter().filter(|ch| ch.joined && ch.kind != "voice").map(|ch| ch.gid)); }
                    gids.truncate(64);
                    let (api, k) = c.net_handle().map_err(e)?;
                    Ok((api, k, gids))
                })?;
                if gids.is_empty() { return Ok(json!({"rooms": {}})); }
                let resp = api.call_presence(&k, &gids).map_err(e)?;
                let people = self.with(|c| Ok(c.call_presence(&resp))).map_err(e)?;
                let mut rooms = serde_json::Map::new();
                for (gid, list) in people {
                    rooms.insert(gid, json!(list.iter().map(|p| json!({"account": p.account, "name": p.name, "me": p.is_me,
                        "muted": p.muted, "deafened": p.deafened, "speaking": p.speaking})).collect::<Vec<_>>()));
                }
                Ok(json!({"rooms": rooms}))
            }
            _ => Err(format!("unknown method {m}")),
        }
    }
}

#[doc(hidden)]
pub fn debug_snapshot(c: &Client) -> Value { snapshot(c) }

/// The page polls `snapshot` and `messages` twice a second, and almost always nothing has changed. Every answer carries a
/// `hash` of itself; a caller that sends the hash it holds as `if` gets `{"same": true}` back when the answer would be the
/// same, instead of the whole thing again. A caller that sends nothing gets the full answer, as before.
fn hash_of(v: &Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.to_string().hash(&mut h);
    h.finish()
}

fn same_or(r: R, a: &Value) -> R {
    let mut v = r?;
    let hash = format!("{:016x}", hash_of(&v));
    if a["if"].as_str() == Some(hash.as_str()) { return Ok(json!({"same": true, "hash": hash})); }
    if let Value::Object(m) = &mut v { m.insert("hash".into(), Value::String(hash)); }
    Ok(v)
}

/// Windows remembered per `messages` key, and keys remembered at all (the open room, and the cards' one look at the
/// newest line of every room, make a key per room and window size).
const WINDOWS_PER_KEY: usize = 4;
const WINDOW_KEYS: usize = 96;

impl App {
    /// The answer to `messages`, sized to what the page already holds. `if` names (by hash) the answer the page has: the
    /// same hash gets "same", as for every read. A page that can take a difference says so (`delta`); when the core
    /// still remembers the rows of the answer it names, it gets only the rows that are new or changed (an edit, a
    /// reaction, a deletion, a line decrypted later), the ids that left the window, and the whole order only when
    /// appending the new rows does not give it; `more`, `typing` and `status` always, since they are small. Applied to
    /// what the page holds, that is exactly the full answer, hash included. A page that does not ask for a difference,
    /// or holds an answer the core no longer remembers, gets the whole window, exactly as before.
    fn messages_answer(&self, key: String, mut v: Value, a: &Value) -> R {
        let hash = format!("{:016x}", hash_of(&v));
        let have = a["if"].as_str();
        if have == Some(hash.as_str()) { return Ok(json!({"same": true, "hash": hash})); }
        let wants_delta = a["delta"].as_bool().unwrap_or(false) || a["delta"].as_u64().is_some_and(|n| n > 0);
        if !wants_delta { // a page from before differences: the whole window, as always
            if let Value::Object(m) = &mut v { m.insert("hash".into(), Value::String(hash)); }
            return Ok(v);
        }
        let rows: Vec<(String, u64)> = v["messages"].as_array().map(|l| l.iter().map(|m| (m["id"].as_str().unwrap_or("").to_string(), hash_of(m))).collect()).unwrap_or_default();
        let old = {
            let mut w = self.windows.lock().unwrap();
            let old = have.and_then(|h| w.get(&key)?.iter().find(|(k, _)| k == h).map(|(_, r)| r.clone()));
            if !w.contains_key(&key) && w.len() >= WINDOW_KEYS { w.clear(); }
            let list = w.entry(key).or_default();
            list.retain(|(k, _)| k != &hash);
            list.push_back((hash.clone(), rows.clone()));
            while list.len() > WINDOWS_PER_KEY { list.pop_front(); }
            old
        };
        let distinct = rows.iter().map(|(id, _)| id).collect::<std::collections::HashSet<_>>().len() == rows.len();
        if let (Some(old), true) = (old, distinct) {
            let now: HashMap<&str, u64> = rows.iter().map(|(id, h)| (id.as_str(), *h)).collect();
            let was: HashMap<&str, u64> = old.iter().map(|(id, h)| (id.as_str(), *h)).collect();
            let msgs = v["messages"].as_array().cloned().unwrap_or_default();
            let changed: Vec<Value> = msgs.iter().filter(|m| { let id = m["id"].as_str().unwrap_or(""); was.get(id) != now.get(id) }).cloned().collect();
            let drop: Vec<&str> = old.iter().map(|(id, _)| id.as_str()).filter(|id| !now.contains_key(id)).collect();
            // What the page builds (its order less the dropped, the new ones appended) against the order that is.
            let mut built: Vec<&str> = old.iter().map(|(id, _)| id.as_str()).filter(|id| now.contains_key(id)).collect();
            built.extend(rows.iter().map(|(id, _)| id.as_str()).filter(|id| !was.contains_key(id)));
            let actual: Vec<&str> = rows.iter().map(|(id, _)| id.as_str()).collect();
            let mut out = json!({"delta": true, "hash": hash, "rows": changed, "drop": drop, "more": v["more"], "typing": v["typing"], "status": v["status"]});
            if built != actual { out["order"] = json!(actual); }
            return Ok(out);
        }
        if let Value::Object(m) = &mut v { m.insert("hash".into(), Value::String(hash)); }
        Ok(v)
    }
}

fn snapshot(c: &Client) -> Value {
    let me = c.my_account();
    let comms: Vec<Value> = c.st.communities.values().filter(|co| !c.is_direct(co)).map(|co| {
        let role = co.role_of(&me).unwrap_or(Role::Member);
        let list = c.channels_of(&co.id);
        let unread: usize = list.iter().map(|x| x.unread).sum();
        let chans: Vec<Value> = list.into_iter().map(|ch| json!({"gid": ch.gid, "name": ch.name, "kind": ch.kind, "joined": ch.joined, "unread": ch.unread, "status": c.group_status(&ch.gid)})).collect();
        let style = co.style.as_ref().map(|d| json!({"ver": d.ver, "ts": d.ts, "by": c.display_name(&co.owner), "community": d.community.as_ref().map(|x| x.lower()).map(|x| json!({"main": x.main, "hi": x.hi})),
            "locked": d.locked.iter().filter(|g| **g == co.id || co.channels.iter().any(|ch| &ch.id == *g)).collect::<Vec<_>>(),
            "looks": d.looks.iter().filter(|(g, _)| **g == co.id || co.channels.iter().any(|ch| &ch.id == *g)).map(|(g, l)| (g.clone(), { let mut v = json!({"card": l.card, "chat": l.chat, "dim": l.dim}); if l.oracle_quiet == Some(true) { v["oracle_quiet"] = json!(true); } if l.oracle_draw == Some(true) { v["oracle_draw"] = json!(true); } v })).collect::<serde_json::Map<String, Value>>(),
            "wall": d.wall.iter().filter(|(g, _)| **g == co.id || co.channels.iter().any(|ch| &ch.id == *g)).map(|(g, n)| (g.clone(), json!(n))).collect::<serde_json::Map<String, Value>>(),
            "rooms": d.rooms.iter().filter(|(g, _)| co.channels.iter().any(|ch| &ch.id == *g)).map(|(g, x)| { let x = x.lower(); (g.clone(), json!({"main": x.main, "hi": x.hi})) }).collect::<serde_json::Map<String, Value>>()}));
        json!({"id": co.id, "style": style, "name": if co.name.is_empty() { "…".to_string() } else { co.name.clone() }, "role": role.name(), "members": c.st.groups.get(&co.id).map(|g| g.accounts.len()).unwrap_or(0), "removed": co.removed, "removed_reason": co.removed_reason, "channels": chans, "unread": unread,
               "banned": if role >= Role::Admin { co.banned.iter().map(|a| json!({"account": a, "name": c.display_name(a)})).collect::<Vec<_>>() } else { vec![] }, "invites": co.invites.iter().map(|i| json!({"id": i.id, "expires": i.expires, "uses_left": i.uses_left, "mine": co.invite_sks.contains_key(&i.id), "password": c.st.pw_invites.contains_key(&i.id)})).collect::<Vec<_>>(),
               "status": c.group_status(&co.id)})
    }).collect();
    let joining: Vec<Value> = c.st.joining.values().filter(|l| !c.st.direct_joining.contains_key(&l.community)).map(|l| json!({"id": l.community, "name": l.name, "sent": c.st.join_sent.contains(&l.community), "asked": c.st.join_asked.get(&l.community)})).collect();
    let refused: Vec<Value> = c.st.join_refused.iter().filter(|(id, _)| !c.st.direct_joining.contains_key(*id)).map(|(id, r)| json!({"id": id, "name": r.name, "reason": r.reason, "ts": r.ts})).collect();
    let problems: Vec<Value> = c.st.notices.iter().rev().filter(|n| matches!(n.kind.as_str(), "decrypt" | "broken" | "rollback" | "reject" | "gap" | "welcome" | "relay" | "state" | "clock" | "revoked")).take(5).map(|n| json!({"ts": n.ts, "kind": n.kind, "text": n.text})).collect();
    // One-to-one calls are never among the communities: they have their own small list, named after the other person.
    let direct: Vec<Value> = c.direct_list().into_iter().map(|d| json!({"id": d.cid, "peer": d.peer, "name": c.display_name(&d.peer), "room": d.room, "last": d.last, "members": d.members, "mine": d.founder_me,
        "joined": c.channels_of(&d.cid).iter().find(|ch| ch.gid == d.room).is_some_and(|ch| ch.joined), "supported": c.direct_support(&d.peer), "direct": true, "removed": false,
        "channels": c.channels_of(&d.cid).into_iter().map(|ch| json!({"gid": ch.gid, "name": ch.name, "kind": ch.kind, "joined": ch.joined, "unread": 0})).collect::<Vec<_>>()})).collect();
    let direct_refused: Vec<Value> = c.st.direct_refused.iter().filter(|a| !a.starts_with("c:")).map(|a| json!({"account": a, "name": c.display_name(a)})).collect();
    let net = match c.net.state { NetState::Connecting => "connecting", NetState::Online => "online", NetState::Offline => "offline" };
    let listening = c.listening_of(&me).map(|l| json!({"title": l.title, "artist": l.artist}));
    json!({"me": {"account": me, "name": c.st.name, "device": c.my_device(), "recovery_unsaved": c.st.recovery_unsaved}, "relay": c.st.relay, "relay_secret": !c.st.relay_invite.is_empty(), "communities": comms, "direct": direct, "direct_refused": direct_refused, "joining": joining, "refused": refused,
           "typing_enabled": c.st.typing_enabled, "share_listening": c.st.share_listening, "rune_url": c.rune_url(), "listening": listening,
           "outbox": c.st.outbox.len(), "problems": problems, "net": net, "net_detail": c.net.detail, "revoked": c.st.self_revoked,
           "download_dir": c.download_dir().display().to_string(),
           "mobile": cfg!(any(target_os = "ios", target_os = "android")), "version": c.activity})
}

/// A web address from a message that may be handed to the system browser: http or https, a host, and only the characters
/// URLs are made of (so nothing that a browser launcher could read as an option or a second argument).
pub fn link_ok(u: &str) -> bool {
    let rest = match u.strip_prefix("https://").or_else(|| u.strip_prefix("http://")) { Some(r) => r, None => return false };
    u.len() <= 2048 && !rest.is_empty() && !rest.starts_with(['/', '.', '-'])
        && u.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._~:/?#[]@!$&'()*+,;=%".contains(&b))
}

/// "Carol joined" and "Carol left", from the lines the sync writes when the room's members change.
fn member_line(c: &Client, t: &str) -> Option<String> {
    if let Some(a) = t.strip_prefix(crate::sync::MEMBER_CAME) { return Some(format!("{} joined the room", c.display_name(a))); }
    t.strip_prefix(crate::sync::MEMBER_WENT).map(|a| format!("{} is no longer in this room", c.display_name(a)))
}

/// The newest `limit` messages of a room, reaching back far enough to include `around` if that is set.
fn messages(c: &Client, gid: &str, limit: usize, around: &str) -> Value {
    let me = c.my_account();
    let all = c.message_log(gid);
    let mut start = all.len().saturating_sub(limit);
    if !around.is_empty() { if let Some(p) = all.iter().position(|m| m.id == around) { start = start.min(p.saturating_sub(20)); } }
    let find: HashMap<&str, &Msg> = all.iter().map(|m| (m.id.as_str(), *m)).collect();
    let who = c.st.groups.get(gid).map(|g| c.people_of(&g.community)).unwrap_or_default();
    let out: Vec<Value> = all[start..].iter().map(|m| {
        let (kind, text, file) = match &m.body {
            MsgBody::Text(t) => ("text", t.clone(), Value::Null),
            MsgBody::File { name, mime, size, caption, large, .. } => {
                let mut f = json!({"name": name, "mime": mime, "size": large.as_ref().map(|l| l.size).unwrap_or(*size)});
                if let Some(l) = large {
                    f["large"] = json!({"size": l.size, "chunks": l.chunks, "chunk_size": l.chunk_size, "sha256": l.sha256, "expires": l.expires});
                    if let Some(d) = c.large_download_state(&m.id) {
                        f["download"] = json!({"done": d.done, "chunks": d.chunks, "size": d.size, "finished": d.finished, "path": d.path, "save_name": d.save_name});
                    }
                }
                ("file", caption.clone(), f)
            }
            MsgBody::Notice(t) => match member_line(c, t) { Some(line) => ("member", line, Value::Null), None => ("notice", t.clone(), Value::Null) },
        };
        let reply = m.reply.as_deref().and_then(|id| find.get(id)).map(|r| json!({"id": r.id, "name": c.display_name(&r.sender), "text": match &r.body { MsgBody::Text(t) if !r.deleted => t.chars().take(120).collect::<String>(), MsgBody::File { name, .. } => format!("[file] {name}"), _ => "(deleted)".into() }}));
        let (role, same_name) = who.get(&m.sender).cloned().unwrap_or((Role::Member, false));
        json!({"id": m.id, "sender": m.sender, "name": if m.sender.is_empty() { String::new() } else { c.display_name(&m.sender) }, "mine": m.sender == me, "ts": m.ts, "seq": m.seq,
               "role": role.name(), "tag": crate::ops::account_tag(&m.sender), "same_name": same_name,
               "status": m.status, "kind": kind, "text": text, "file": file, "reply": reply, "edited": m.edited, "deleted": m.deleted,
               "reactions": m.reactions.iter().map(|(e, who)| json!({"emoji": e, "count": who.len(), "mine": who.contains(&me), "names": who.iter().map(|w| c.display_name(w)).collect::<Vec<_>>()})).collect::<Vec<_>>()})
    }).collect();
    json!({"messages": out, "more": start > 0, "typing": c.typing_now(gid), "status": c.group_status(gid)})
}

#[cfg(test)]
mod messages_answer_tests {
    use super::*;
    fn app() -> (Arc<App>, tempfile::TempDir) { let d = tempfile::tempdir().unwrap(); (App::with_kdf(d.path().to_path_buf(), "", KdfParams::TEST), d) }
    fn row(id: &str, text: &str) -> Value { json!({"id": id, "text": text, "reactions": [], "edited": false, "deleted": false}) }
    fn window(rows: &[Value], typing: &[&str]) -> Value { json!({"messages": rows, "more": false, "typing": typing, "status": "ok"}) }
    /// What app.js `mergeMessages` does with a difference.
    fn merge(have: &Value, d: &Value) -> Value {
        let gone: Vec<&str> = d["drop"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
        let mut list: Vec<Value> = have["messages"].as_array().unwrap().iter().filter(|x| !gone.contains(&x["id"].as_str().unwrap())).cloned().collect();
        for r in d["rows"].as_array().unwrap() {
            match list.iter().position(|x| x["id"] == r["id"]) { Some(i) => list[i] = r.clone(), None => list.push(r.clone()) }
        }
        if let Some(o) = d["order"].as_array() { list = o.iter().map(|id| list.iter().find(|x| x["id"] == *id).unwrap().clone()).collect(); }
        json!({"messages": list, "more": d["more"], "typing": d["typing"], "status": d["status"], "hash": d["hash"]})
    }
    fn full_with_hash(v: &Value) -> Value { let mut f = v.clone(); f["hash"] = json!(format!("{:016x}", hash_of(v))); f }

    #[test]
    fn a_page_from_before_gets_the_whole_window_and_a_new_page_only_the_difference() {
        let (app, _d) = app();
        let key = "messages/g/200/".to_string();
        let w1 = window(&[row("a", "one"), row("b", "two"), row("c", "three")], &[]);
        // An old page: never says `delta`, gets the window with its hash, then "same".
        let full = app.messages_answer(key.clone(), w1.clone(), &json!({})).unwrap();
        assert_eq!(full, full_with_hash(&w1));
        let h1 = full["hash"].as_str().unwrap().to_string();
        assert_eq!(app.messages_answer(key.clone(), w1.clone(), &json!({"if": h1})).unwrap(), json!({"same": true, "hash": h1}));
        // The old page holds h1 but a change comes: the whole window, as always (nothing was remembered for it).
        let w2 = window(&[row("a", "one"), row("b", "two"), row("c", "three")], &["bob"]);
        let full2 = app.messages_answer(key.clone(), w2.clone(), &json!({"if": h1})).unwrap();
        assert_eq!(full2, full_with_hash(&w2));
        // A new page: the first answer is the window; from then on differences, each of which rebuilds the full answer.
        let page = app.messages_answer(key.clone(), w1.clone(), &json!({"delta": 1})).unwrap();
        assert_eq!(page, full_with_hash(&w1));
        let mut have = page;
        let steps = [
            window(&[row("a", "one"), row("b", "two"), row("c", "three")], &["bob"]),                 // typing only
            window(&[row("a", "one"), row("b", "two"), row("c", "three"), row("d", "four")], &[]),  // a new message
            window(&[row("a", "one"), row("b", "TWO"), row("c", "three"), row("d", "four")], &[]),  // an edit
            window(&[row("b", "TWO"), row("c", "three"), row("d", "four"), row("e", "five")], &[]), // the window slid
            window(&[row("b", "TWO"), row("c", "three"), row("x", "late"), row("d", "four"), row("e", "five")], &[]), // decrypted later, in the middle
        ];
        for (i, w) in steps.iter().enumerate() {
            let d = app.messages_answer(key.clone(), w.clone(), &json!({"delta": 1, "if": have["hash"]})).unwrap();
            assert_eq!(d["delta"], true, "step {i}: a difference, not the window");
            let rows = d["rows"].as_array().unwrap().len();
            assert!(rows <= 1, "step {i}: at most one row changed, got {rows}");
            assert_eq!(d["order"].is_array(), i == 4, "step {i}: the order travels only when appending does not give it");
            have = merge(&have, &d);
            assert_eq!(have, full_with_hash(w), "step {i}: the page holds exactly the full answer");
        }
        // An answer the core no longer remembers (another room was open meanwhile, many times): the whole window.
        for k in 0..WINDOWS_PER_KEY + 1 { app.messages_answer(key.clone(), window(&[row("z", &k.to_string())], &[]), &json!({"delta": 1})).unwrap(); }
        let w = window(&[row("b", "TWO")], &[]);
        assert_eq!(app.messages_answer(key.clone(), w.clone(), &json!({"delta": 1, "if": have["hash"]})).unwrap(), full_with_hash(&w));
    }
}

#[cfg(test)]
mod link_tests {
    use super::link_ok;
    #[test]
    fn only_plain_web_addresses_are_opened() {
        for good in ["https://example.org", "http://127.0.0.1:8787/x?a=1&b=2#top", "https://en.wikipedia.org/wiki/Catan_(game)", "https://x.com/@alice"] { assert!(link_ok(good), "{good}"); }
        for bad in ["", "file:///etc/passwd", "javascript:alert(1)", "https://", "https://-x", "https:///x", "https://a b", "https://a\"b", "https://a\nb", "https://ä.example", "ftp://x", "https://a`b", "https://a<b>", "https://a|b", "https://a\\b"] { assert!(!link_ok(bad), "{bad:?}"); }
        assert!(!link_ok(&format!("https://{}", "a".repeat(2050))));
    }
}
