//! The app's side of listening along: joining a friend, the login to Spotify, and the once-a-second step (`App::along_tick`).
//! The decisions and the HTTP are in spotify.rs; this only connects them to the client's state and the page.
use crate::api::App;
use crate::spotify::{self, Backend, Mem, RuneTarget, Sample, SpotifyTarget};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

type R = Result<Value, String>;

struct Session { gen: u64, acct: String, name: String, mem: Mem, be: Box<dyn Backend>, target: String, title: String }
struct Pending { verifier: String, state: String, redirect: String, at: Instant }

#[derive(Default)]
pub struct Along {
    /// Taken out while a step does its network calls, so the page's questions never wait for them.
    sess: Mutex<Option<Session>>,
    status: Mutex<Value>,
    /// Bumped by every join and leave, so a step that finished after the person left puts nothing back.
    gen: AtomicU64,
    pending: Mutex<Option<Pending>>,
}

fn mobile() -> bool { cfg!(any(target_os = "ios", target_os = "android")) }
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> { m.lock().unwrap_or_else(|p| p.into_inner()) }

impl App {
    fn along_set(&self, v: Value) { *lock(&self.along.status) = v; }

    pub(crate) fn along_rpc(&self, m: &str, a: &Value) -> R {
        let s = |k: &str| a.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        match m {
            "listen_status" => Ok(self.along_status()),
            // What Join can do on this device: the rune zones that answer, and whether Spotify itself is set up.
            "listen_targets" => {
                let (url, cid, auth) = self.with(|c| Ok((c.rune_url(), spotify::client_id(&c.st.spotify_client_id), c.st.spotify.is_some())))?;
                let zones = crate::rune::zones(&url).unwrap_or_default();
                Ok(json!({"zones": zones.iter().map(|(id, name)| json!({"id": id, "name": name})).collect::<Vec<_>>(),
                          "spotify": !mobile() && !cid.is_empty() && auth, "can_login": !mobile() && !cid.is_empty(), "set_up": !cid.is_empty(), "mobile": mobile()}))
            }
            "listen_leave" => {
                self.along.gen.fetch_add(1, Ordering::SeqCst);
                *lock(&self.along.sess) = None;
                self.along_set(json!({"state": "off"}));
                Ok(json!({}))
            }
            "listen_join" => self.along_join(&s("account"), &s("target")),
            "spotify_config" => self.with(|c| Ok(json!({
                "client_id": c.st.spotify_client_id, "effective": !spotify::client_id(&c.st.spotify_client_id).is_empty(),
                "logged_in": c.st.spotify.is_some(), "mobile": mobile(),
            }))),
            "spotify_set_client_id" => {
                let id = s("id").trim().to_string();
                if id.len() > 64 || !id.bytes().all(|c| c.is_ascii_alphanumeric()) { return Err("That does not look like a Spotify client ID (letters and numbers only).".into()); }
                self.with(|c| {
                    if c.st.spotify_client_id != id { c.st.spotify = None; c.st.spotify_client_id = id; }
                    c.save().map_err(|e| e.to_string())?;
                    Ok(json!({}))
                })
            }
            "spotify_logout" => { self.with(|c| { c.st.spotify = None; c.save().map_err(|e| e.to_string()) })?; Ok(json!({})) }
            "spotify_login_start" => {
                if mobile() { return Err("Logging in to Spotify is not offered on a phone.".into()); }
                let redirect = s("redirect");
                if !spotify::redirect_ok(&redirect) { return Err("Spotify can only send you back to this app's own address.".into()); }
                let cid = self.with(|c| Ok(spotify::client_id(&c.st.spotify_client_id)))?;
                if cid.is_empty() { return Err("Put the Spotify client ID in Settings first.".into()); }
                let (verifier, state) = (spotify::new_verifier(), spotify::new_state());
                let url = spotify::auth_url(&spotify::Endpoints::from_env(), &cid, &redirect, &spotify::challenge(&verifier), &state);
                *lock(&self.along.pending) = Some(Pending { verifier, state, redirect, at: Instant::now() });
                Ok(json!({"url": url}))
            }
            _ => Err(format!("unknown call {m}")),
        }
    }

    fn along_status(&self) -> Value { let v = lock(&self.along.status).clone(); if v.is_null() { json!({"state": "off"}) } else { v } }

    fn along_join(&self, acct: &str, target: &str) -> R {
        let (sample, name, url, cid, auth, me) = self.with(|c| Ok((c.listening_sample(acct), c.display_name(acct), c.rune_url(), spotify::client_id(&c.st.spotify_client_id), c.st.spotify.clone(), c.my_account())))?;
        if acct == me { return Err("That is you.".into()); }
        let Some((np, _)) = sample else { return Err(format!("{name} is not sharing a song right now.")) };
        let Some(sp) = np.spotify.as_ref() else { return Err(format!("{name} is not playing a Spotify song, so there is nothing to join.")) };
        let link = spotify::open_link(&sp.id);
        // The person's own rune first (it plays through its own engine), then their Spotify account.
        let mut be: Option<Box<dyn Backend>> = None;
        if target.is_empty() || target.starts_with("rune:") {
            if let Ok(zones) = crate::rune::zones(&url) {
                let want = target.strip_prefix("rune:").unwrap_or("");
                let z = zones.iter().find(|(id, _)| !want.is_empty() && id == want).or_else(|| zones.iter().find(|(id, _)| id == "pc")).or_else(|| zones.first());
                if let Some((id, zname)) = z { be = Some(Box::new(RuneTarget { base: url.clone(), zone: id.clone(), name: zname.clone() })); }
            }
        }
        if be.is_none() && !mobile() && (target.is_empty() || target == "spotify") {
            if let (false, Some(auth)) = (cid.is_empty(), auth) { be = Some(Box::new(SpotifyTarget::new(spotify::Endpoints::from_env(), cid.clone(), auth))); }
        }
        let gen = self.along.gen.fetch_add(1, Ordering::SeqCst) + 1;
        let Some(be) = be else {
            *lock(&self.along.sess) = None;
            let (needs, msg) = if mobile() || cid.is_empty() { ("open", "Open the song in Spotify to listen.") } else { ("login", "Log in to Spotify to listen along here.") };
            let st = json!({"state": "needs", "who": name, "acct": acct, "title": np.title, "needs": needs, "message": msg, "open_url": link});
            self.along_set(st.clone());
            return Ok(st);
        };
        let label = be.label();
        *lock(&self.along.sess) = Some(Session { gen, acct: acct.into(), name: name.clone(), mem: Mem::default(), be, target: label.clone(), title: np.title.clone() });
        let st = json!({"state": "on", "who": name, "acct": acct, "title": np.title, "target": label, "message": "", "needs": "", "open_url": link});
        self.along_set(st.clone());
        Ok(st)
    }

    /// One step, once a second. Does nothing unless the person joined a friend.
    pub(crate) fn along_tick(&self) {
        let Some(mut s) = lock(&self.along.sess).take() else { return };
        let end = |this: &App, s: &Session, why: String, needs: &str| {
            // Leave the person's own player as it is: only the following stops.
            this.along_set(json!({"state": "ended", "who": s.name, "acct": s.acct, "title": s.title, "message": why, "needs": needs}));
        };
        let heard = match self.with(|c| Ok(c.listening_sample(&s.acct))) { Ok(h) => h, Err(_) => { self.along_put_back(s); return } };
        let now = spotify::now_ms();
        let Some((np, rx_ms)) = heard else { end(self, &s, format!("{} stopped sharing. You are no longer listening along.", s.name), ""); return };
        let Some(play) = np.spotify.clone() else { end(self, &s, format!("{} switched to a song that is not on Spotify. You are no longer listening along.", s.name), ""); return };
        s.title = np.title.clone();
        let out = spotify::step(&mut s.mem, &mut *s.be, &Sample { play: play.clone(), rx_ms }, now);
        // A refreshed login is kept (or dropped, when Spotify refused it).
        if let Some(auth) = s.be.auth() {
            let _ = self.with(|c| { c.st.spotify = Some(auth); c.save().map_err(|e| e.to_string()) });
        }
        if out.ended {
            if out.needs == "login" { let _ = self.with(|c| { c.st.spotify = None; c.save().map_err(|e| e.to_string()) }); }
            let mut st = json!({"state": "ended", "who": s.name, "acct": s.acct, "title": s.title, "message": out.message, "needs": out.needs});
            if let Some(l) = spotify::open_link(&play.id) { st["open_url"] = json!(l); }
            if self.along.gen.load(Ordering::SeqCst) == s.gen { self.along_set(st); }
            return;
        }
        if self.along.gen.load(Ordering::SeqCst) != s.gen { return; }
        // Only a message that changed is news; an ordinary step leaves the status as it was.
        let was = self.along_status();
        let st = json!({"state": "on", "who": s.name, "acct": s.acct, "title": s.title, "target": s.target, "paused": play.paused, "message": out.message, "needs": out.needs, "open_url": spotify::open_link(&play.id)});
        if was != st { self.along_set(st); }
        self.along_put_back(s);
    }

    fn along_put_back(&self, s: Session) {
        if self.along.gen.load(Ordering::SeqCst) == s.gen { *lock(&self.along.sess) = Some(s); }
    }

    /// The browser's return from Spotify's login page: `GET /spotify/callback?code=..&state=..`. Answers with a small page.
    pub fn spotify_callback(&self, query: &str) -> (u16, String) {
        let page = |ok: bool, msg: &str| (if ok { 200 } else { 400 }, format!("<!doctype html><meta charset=utf-8><title>mega</title><body style=\"font:16px system-ui;max-width:32em;margin:4em auto;padding:0 1em\"><h2>{}</h2><p>{}</p>", if ok { "Spotify is connected" } else { "Spotify did not connect" }, msg));
        let q = spotify::parse_query(query);
        let pending = lock(&self.along.pending).take();
        let Some(p) = pending.filter(|p| p.at.elapsed() < Duration::from_secs(600) && q.get("state").is_some_and(|s| *s == p.state)) else { return page(false, "This login was not started here, or it took too long. Start it again from mega's settings.") };
        if q.contains_key("error") || !q.contains_key("code") { return page(false, "Spotify did not give permission. Nothing was changed."); }
        let cid = match self.with(|c| Ok(spotify::client_id(&c.st.spotify_client_id))) { Ok(c) if !c.is_empty() => c, _ => return page(false, "mega is locked or has no Spotify client ID.") };
        match spotify::exchange_code(&spotify::Endpoints::from_env(), &cid, &q["code"], &p.redirect, &p.verifier) {
            Ok(auth) => {
                let _ = self.with(|c| { c.st.spotify = Some(auth); c.save().map_err(|e| e.to_string()) });
                page(true, "You can close this tab and go back to mega.")
            }
            Err(_) => page(false, "Spotify refused the login. Check the client ID and the redirect address in your Spotify app."),
        }
    }
}
