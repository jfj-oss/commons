//! JSON dispatch used by both the Tauri shell and the local dev bridge: the UI talks to the Rust core only
//! through `App::call(method, args)`. Keys and plaintext never leave this process.
use crate::client::*;
use crate::envelope::{check_name, safe_file_name, InviteLink, RelayLink};
use crate::state::*;
use crate::transport::{Api, HttpTransport};
use crate::vault::KdfParams;
use commons_proto::{b64, unb64};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, TryLockError};
use std::time::Duration;

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
    stop: Mutex<bool>,
    /// The last answer to each read-only call. The UI asks twice a second, and gets this if the client is busy.
    seen: Mutex<HashMap<String, Value>>,
    files: Mutex<Vec<(String, Arc<Vec<u8>>)>>,
    /// loopback server for "open this call in my browser" (started on first use)
    bridge: Mutex<Option<crate::callbridge::CallBridge>>,
    /// Per-game push-to-mute. Runs only while a call keeps asking.
    game: commons_gamekeys::Service,
    /// When a call last asked the core for something (its key poll, token, or game-key heartbeat), from this window or
    /// from a call handed off to the browser. The updater waits while this is recent: `call_active`.
    call_seen: Mutex<Option<std::time::Instant>>,
    me: std::sync::Weak<App>,
}

/// A running call asks the core at least every few seconds (call.js: the key poll every 1.5 s, the game-key heartbeat
/// at most 4 s apart), so this much silence means no call is running on this device.
pub const CALL_QUIET: Duration = Duration::from_secs(20);

fn s(v: &Value, k: &str) -> String { v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string() }
type R = Result<Value, String>;
fn e<T: std::fmt::Display>(x: T) -> String { format!("{x}") }

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
        Arc::new_cyclic(|me| App { me: me.clone(), call_seen: Mutex::new(None), bridge: Mutex::new(None), game: commons_gamekeys::Service::new(), dir, default_relay: default_relay.into(), kdf, client: Mutex::new(None), wake: (Mutex::new(false), Condvar::new()), stop: Mutex::new(false), seen: Mutex::new(HashMap::new()), files: Mutex::new(vec![]) })
    }
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
        // The browser bridge's listener thread holds the app alive, so it has to be ended here.
        let bridge = self.bridge.lock().unwrap().take();
        if let Some(b) = bridge { b.stop(); }
    }
    fn saw_call(&self) { *self.call_seen.lock().unwrap_or_else(|p| p.into_inner()) = Some(std::time::Instant::now()); }
    /// True while a call on this device is running (in this window or in the browser): an update must not restart the app now.
    pub fn call_active(&self) -> bool {
        self.call_seen.lock().unwrap_or_else(|p| p.into_inner()).is_some_and(|t| t.elapsed() < CALL_QUIET)
    }
    #[doc(hidden)]
    pub fn bridge_running(&self) -> bool { self.bridge.lock().unwrap().as_ref().is_some_and(|b| !b.is_stopped()) }
    fn nudge(&self) { *self.wake.0.lock().unwrap() = true; self.wake.1.notify_all(); }
    /// A panic in one call must not take every later call down with it.
    fn lock(&self) -> MutexGuard<'_, Option<Client>> { self.client.lock().unwrap_or_else(|p| p.into_inner()) }

    /// Background sync loop: every second (or immediately after a user action). The client is held
    /// only to plan a round and to apply its result, never while waiting for the relay.
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
    }

    pub fn call(&self, method: &str, a: Value) -> R {
        let in_call = matches!(method, "call_state" | "call_token" | "game_mute_wait");
        if in_call { self.saw_call(); }
        let r = self.dispatch(method, a);
        if in_call && r.is_ok() { self.saw_call(); }
        if !matches!(method, "snapshot" | "messages" | "state" | "file_read" | "pairing_status" | "game_mute_wait" | "call_presence") { self.nudge(); }
        r
    }

    fn with<T>(&self, f: impl FnOnce(&mut Client) -> Result<T, String>) -> Result<T, String> {
        let mut g = self.lock();
        match g.as_mut() { Some(c) => f(c), None => Err("locked".into()) }
    }

    /// A read-only call. If the client is busy with something long (an admin action waiting for the relay),
    /// the last answer is good enough: the UI must not freeze for that.
    fn read(&self, key: String, f: impl FnOnce(&Client) -> Value) -> R {
        let g = match self.client.try_lock() {
            Ok(g) => g,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => {
                if let Some(v) = self.seen.lock().unwrap().get(&key) { return Ok(v.clone()); }
                self.lock()
            }
        };
        let v = f(g.as_ref().ok_or("locked")?);
        self.seen.lock().unwrap().insert(key, v.clone());
        Ok(v)
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
            "state" => {
                let g = self.lock();
                match g.as_ref() {
                    None => Ok(json!({"locked": true, "exists": Client::exists(&self.dir), "default_relay": self.default_relay})),
                    Some(c) => Ok(json!({"locked": false, "has_identity": c.has_identity(), "relay": c.st.relay})),
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
                self.files.lock().unwrap().clear();
                Ok(json!({"ok": true}))
            }
            "snapshot" => self.read("snapshot".into(), snapshot),
            "messages" => {
                let (gid, around) = (s(&a, "gid"), s(&a, "around"));
                let limit = a.get("limit").and_then(|x| x.as_u64()).unwrap_or(200).clamp(1, 5000) as usize;
                self.read(format!("messages/{gid}/{limit}/{around}"), |c| messages(c, &gid, limit, &around))
            }
            "mark_read" => self.with(|c| { c.mark_read(&s(&a, "gid")); c.save().map_err(e)?; Ok(json!({})) }),
            "send_text" => self.with(|c| { let r = a.get("reply").and_then(|x| x.as_str()).map(|x| x.to_string()); c.send_text(&s(&a, "gid"), &s(&a, "text"), r).map(|id| json!({"id": id})).map_err(e) }),
            "edit" => self.with(|c| c.edit_message(&s(&a, "gid"), &s(&a, "id"), &s(&a, "text")).map(|_| json!({})).map_err(e)),
            "delete" => self.with(|c| c.delete_message(&s(&a, "gid"), &s(&a, "id")).map(|_| json!({})).map_err(e)),
            "react" => self.with(|c| c.react(&s(&a, "gid"), &s(&a, "id"), &s(&a, "emoji"), a["on"].as_bool().unwrap_or(true)).map(|_| json!({})).map_err(e)),
            "typing" => self.with(|c| { c.send_typing(&s(&a, "gid")); Ok(json!({})) }),
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
            "create_community" => self.with(|c| c.create_community(&s(&a, "name")).map(|id| json!({"id": id})).map_err(e)),
            "create_channel" => self.with(|c| c.create_channel(&s(&a, "cid"), &s(&a, "name")).map(|id| json!({"id": id})).map_err(e)),
            "create_invite" => self.with(|c| {
                let uses = a.get("uses").and_then(|x| x.as_u64()).map(|x| x as u32);
                let ttl = a.get("ttl").and_then(|x| x.as_u64()).unwrap_or(7 * 86400);
                let link = c.create_invite(&s(&a, "cid"), uses, ttl).map_err(e)?;
                let web = InviteLink::decode(&link).ok().and_then(|l| l.web());
                Ok(json!({"link": web.as_deref().unwrap_or(&link), "qr": Client::qr_svg(web.as_deref().unwrap_or(&link))}))
            }),
            "join" => self.with(|c| c.join_with_link(&s(&a, "link")).map(|id| json!({"id": id})).map_err(e)),
            "cancel_join" => self.with(|c| c.cancel_join(&s(&a, "id")).map(|_| json!({})).map_err(e)),
            "kick" => self.with(|c| c.kick(&s(&a, "cid"), &s(&a, "account")).map(|_| json!({})).map_err(e)),
            "set_role" => self.with(|c| c.set_role(&s(&a, "cid"), &s(&a, "account"), if s(&a, "role") == "admin" { Role::Admin } else { Role::Member }).map(|_| json!({})).map_err(e)),
            "transfer_ownership" => self.with(|c| c.transfer_ownership(&s(&a, "cid"), &s(&a, "account")).map(|_| json!({})).map_err(e)),
            "leave" => self.with(|c| c.leave(&s(&a, "cid")).map(|_| json!({})).map_err(e)),
            "rename_community" => self.with(|c| c.rename_community(&s(&a, "cid"), &s(&a, "name")).map(|_| json!({})).map_err(e)),
            "revoke_invite" => self.with(|c| c.revoke_invite(&s(&a, "cid"), &s(&a, "id")).map(|_| json!({})).map_err(e)),
            "members" => { let cid = s(&a, "cid"); self.read(format!("members/{cid}"), |c| json!(c.members(&cid).into_iter().map(|m| json!({"account": m.account, "name": m.name, "role": m.role.name(), "devices": m.devices, "verified": m.verified, "me": m.is_me, "tag": m.tag, "same_name": m.same_name})).collect::<Vec<_>>())) }
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
            "set_typing_enabled" => self.with(|c| { c.st.typing_enabled = a["on"].as_bool().unwrap_or(false); c.save().map_err(e)?; Ok(json!({})) }),
            "notices" => self.read("notices".into(), |c| json!(c.st.notices.iter().rev().take(80).map(|n| json!({"ts": n.ts, "kind": n.kind, "text": n.text})).collect::<Vec<_>>())),
            "call_state" => self.with(|c| {
                let st = c.call_state(&s(&a, "gid")).map_err(e)?;
                let keys: serde_json::Map<String, Value> = st.keys.iter().map(|(d, k)| (d.clone(), json!(b64(k)))).collect();
                Ok(json!({"epoch": st.epoch, "me": st.my_device, "keys": keys,
                          "members": st.members.iter().map(|m| json!({"device": m.device, "account": m.account, "name": m.name, "me": m.is_me})).collect::<Vec<_>>()}))
            }),
            "game_keys_get" => {
                let settings = self.with(|c| Ok(c.st.game_keys.clone()))?;
                let i = commons_gamekeys::backend::info(&settings);
                Ok(json!({"settings": serde_json::to_value(&settings).map_err(e)?, "backend": i.backend, "hold": i.hold, "toggle": i.toggle, "mouse": i.mouse, "note": i.note, "permission": i.permission, "portal_possible": cfg!(target_os = "linux"), "error": self.game.status().error}))
            }
            "game_keys_set" => {
                let settings: commons_gamekeys::Settings = serde_json::from_value(a.clone()).map_err(|_| "Those settings could not be read.".to_string())?;
                settings.check()?;
                self.with(|c| { c.st.game_keys = settings.clone(); c.save().map_err(e) })?;
                self.game.set_settings(settings);
                Ok(json!({}))
            }
            // Names only, for the picker. It goes no further than this device's own settings page.
            "running_programs" => Ok(json!(commons_gamekeys::backend::processes())),
            "game_keys_allow" => Ok(json!({"allowed": commons_gamekeys::backend::request_permission()})),
            // A call's heartbeat and its news. It waits outside the client lock: this must not hold anything up, or be held up.
            "game_mute_wait" => {
                if let Ok(g) = self.client.try_lock() { self.game.set_settings(g.as_ref().map(|c| c.st.game_keys.clone()).unwrap_or_default()); }
                let since = a.get("since").and_then(|x| x.as_u64()).unwrap_or(0);
                let st = self.game.wait(since, Duration::from_secs(4));
                Ok(json!({"seq": st.seq, "muted": st.muted, "game": st.game, "running": st.running, "error": st.error}))
            }
            "game_mute_clear" => { self.game.user_unmute(); Ok(json!({})) }
            "open_call_in_browser" => self.open_call_in_browser(&s(&a, "gid"), &s(&a, "name"), a["launch"].as_bool().unwrap_or(true)),
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
                let cid = s(&a, "cid");
                let (api, k, gids) = self.with(|c| {
                    let gids: Vec<String> = c.channels_of(&cid).into_iter().filter(|ch| ch.joined).map(|ch| ch.gid).collect();
                    let (api, k) = c.net_handle().map_err(e)?;
                    Ok((api, k, gids))
                })?;
                if gids.is_empty() { return Ok(json!({"rooms": {}})); }
                let resp = api.call_presence(&k, &gids).map_err(e)?;
                let people = self.with(|c| Ok(c.call_presence(&resp))).map_err(e)?;
                let mut rooms = serde_json::Map::new();
                for (gid, list) in people {
                    rooms.insert(gid, json!(list.iter().map(|p| json!({"account": p.account, "name": p.name, "me": p.is_me})).collect::<Vec<_>>()));
                }
                Ok(json!({"rooms": rooms}))
            }
            _ => Err(format!("unknown method {m}")),
        }
    }
}

fn snapshot(c: &Client) -> Value {
    let me = c.my_account();
    let comms: Vec<Value> = c.st.communities.values().map(|co| {
        let role = co.role_of(&me).unwrap_or(Role::Member);
        let list = c.channels_of(&co.id);
        let unread: usize = list.iter().map(|x| x.unread).sum();
        let chans: Vec<Value> = list.into_iter().map(|ch| json!({"gid": ch.gid, "name": ch.name, "joined": ch.joined, "unread": ch.unread, "status": c.group_status(&ch.gid)})).collect();
        json!({"id": co.id, "name": if co.name.is_empty() { "…".to_string() } else { co.name.clone() }, "role": role.name(), "removed": co.removed, "removed_reason": co.removed_reason, "channels": chans, "unread": unread,
               "invites": co.invites.iter().map(|i| json!({"id": i.id, "expires": i.expires, "uses_left": i.uses_left, "mine": co.invite_sks.contains_key(&i.id)})).collect::<Vec<_>>(),
               "status": c.group_status(&co.id)})
    }).collect();
    let joining: Vec<Value> = c.st.joining.values().map(|l| json!({"id": l.community, "name": l.name, "sent": c.st.join_sent.contains(&l.community), "asked": c.st.join_asked.get(&l.community)})).collect();
    let refused: Vec<Value> = c.st.join_refused.iter().map(|(id, r)| json!({"id": id, "name": r.name, "reason": r.reason, "ts": r.ts})).collect();
    let problems: Vec<Value> = c.st.notices.iter().rev().filter(|n| matches!(n.kind.as_str(), "decrypt" | "broken" | "rollback" | "reject" | "gap" | "welcome" | "relay" | "state" | "clock" | "revoked")).take(5).map(|n| json!({"ts": n.ts, "kind": n.kind, "text": n.text})).collect();
    let net = match c.net.state { NetState::Connecting => "connecting", NetState::Online => "online", NetState::Offline => "offline" };
    json!({"me": {"account": me, "name": c.st.name, "device": c.my_device()}, "relay": c.st.relay, "communities": comms, "joining": joining, "refused": refused,
           "typing_enabled": c.st.typing_enabled, "outbox": c.st.outbox.len(), "problems": problems, "net": net, "net_detail": c.net.detail, "revoked": c.st.self_revoked,
           "version": c.activity})
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
            MsgBody::File { name, mime, size, caption, .. } => ("file", caption.clone(), json!({"name": name, "mime": mime, "size": size})),
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
mod link_tests {
    use super::link_ok;
    #[test]
    fn only_plain_web_addresses_are_opened() {
        for good in ["https://example.org", "http://127.0.0.1:8787/x?a=1&b=2#top", "https://en.wikipedia.org/wiki/Catan_(game)", "https://x.com/@alice"] { assert!(link_ok(good), "{good}"); }
        for bad in ["", "file:///etc/passwd", "javascript:alert(1)", "https://", "https://-x", "https:///x", "https://a b", "https://a\"b", "https://a\nb", "https://ä.example", "ftp://x", "https://a`b", "https://a<b>", "https://a|b", "https://a\\b"] { assert!(!link_ok(bad), "{bad:?}"); }
        assert!(!link_ok(&format!("https://{}", "a".repeat(2050))));
    }
}
