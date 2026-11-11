//! Listening along with a friend's Spotify song.
//!
//! The friend's app tells the room which Spotify track it plays and where (`NowPlaying::spotify`). Joining makes the SAME song
//! play on the joiner's own account: through the person's rune when one answers, otherwise through the Spotify Web API with a
//! login that lives only in this device's encrypted state. mega never captures or forwards any audio, and nothing here goes to
//! the relay or into a message.
//!
//! The decisions are pure (`decide`); the HTTP is behind `Backend`, so tests run against fake servers and never the real ones.
use crate::envelope::{is_spotify_id, SpotifyPlay, SEEK_SLACK_MS};
use crate::state::SpotifyAuth;
use base64::Engine as _;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::time::Duration;

pub const SCOPES: &str = "user-modify-playback-state user-read-playback-state";
/// Checks of the friend's own player happen no more often than this, and a call is never repeated sooner after a failure.
pub const CHECK_MS: u64 = 15_000;
/// Further than this from the sharer and we seek.
pub const DRIFT_MS: u64 = 2_000;
const MAX_LATENCY_MS: u64 = 1_500;
const TIMEOUT: Duration = Duration::from_secs(8);

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

// ---------------------------------------------------------------------------------------------------------------- PKCE
fn b64url(b: &[u8]) -> String { base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b) }
fn random_b64(n: usize) -> String {
    use rand::RngCore;
    let mut b = vec![0u8; n];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b64url(&b)
}
/// A code verifier: 43 to 128 unreserved characters (RFC 7636). 64 here.
pub fn new_verifier() -> String { random_b64(48) }
/// The S256 challenge for a verifier.
pub fn challenge(verifier: &str) -> String { b64url(&Sha256::digest(verifier.as_bytes())) }
/// An unguessable `state`, tying the browser's return to the login this device started.
pub fn new_state() -> String { random_b64(18) }

/// Where Spotify may send the browser back: this app's own loopback address, any port, the one path.
pub fn redirect_ok(r: &str) -> bool {
    let Some(rest) = r.strip_prefix("http://127.0.0.1:") else { return false };
    let Some((port, path)) = rest.split_once('/') else { return false };
    port.parse::<u16>().is_ok_and(|p| p > 0) && path == "spotify/callback"
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| match b { b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(), _ => format!("%{b:02X}") }).collect()
}
fn dec(s: &str) -> String {
    let b = s.as_bytes();
    let (mut out, mut i) = (Vec::new(), 0);
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() && s.is_char_boundary(i + 1) && s.is_char_boundary(i + 3) => {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) { out.push(v); i += 3; continue; }
                out.push(b'%'); i += 1;
            }
            b'+' => { out.push(b' '); i += 1; }
            c => { out.push(c); i += 1; }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
pub fn parse_query(q: &str) -> HashMap<String, String> {
    q.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (dec(k), dec(v))).collect()
}

#[derive(Clone, Debug)]
pub struct Endpoints { pub accounts: String, pub api: String }
impl Endpoints {
    /// The real Spotify, unless a test points these at a fake server.
    pub fn from_env() -> Endpoints {
        let get = |k: &str, d: &str| std::env::var(k).ok().filter(|v| v.starts_with("http://127.0.0.1")).unwrap_or_else(|| d.to_string());
        Endpoints { accounts: get("MEGATRON_SPOTIFY_ACCOUNTS_URL", "https://accounts.spotify.com"), api: get("MEGATRON_SPOTIFY_API_URL", "https://api.spotify.com") }
    }
}

/// The address the person opens to approve mega.
pub fn auth_url(ep: &Endpoints, client_id: &str, redirect: &str, challenge: &str, state: &str) -> String {
    format!("{}/authorize?client_id={}&response_type=code&redirect_uri={}&code_challenge_method=S256&code_challenge={}&state={}&scope={}",
        ep.accounts, enc(client_id), enc(redirect), enc(challenge), enc(state), enc(SCOPES))
}

/// The client id in use: the one saved in settings, else MEGATRON_SPOTIFY_CLIENT_ID. Empty means not set up.
pub fn client_id(saved: &str) -> String {
    let s = saved.trim();
    let id = if s.is_empty() { std::env::var("MEGATRON_SPOTIFY_CLIENT_ID").unwrap_or_default() } else { s.to_string() };
    let id = id.trim().to_string();
    if id.len() <= 64 && id.bytes().all(|c| c.is_ascii_alphanumeric()) { id } else { String::new() }
}

// ---------------------------------------------------------------------------------------------------------------- HTTP
#[derive(Debug, Clone, PartialEq)]
pub enum Fail {
    /// Spotify has no device to play on.
    NoDevice,
    /// The account is not Premium (or Spotify refuses control of it).
    Premium,
    /// Asked to wait this many seconds.
    Limited(u64),
    /// The login is gone or refused; the person has to log in again.
    Auth,
    Other(String),
}

struct Resp { status: u16, body: String, retry_after: Option<u64> }
enum Body<'a> { None, Json(&'a serde_json::Value), Form(&'a [(&'a str, &'a str)]) }

fn send(agent: &ureq::Agent, method: &str, url: &str, bearer: Option<&str>, body: Body) -> Result<Resp, String> {
    let mut req = agent.request(method, url);
    if let Some(t) = bearer { req = req.set("Authorization", &format!("Bearer {t}")); }
    let r = match body {
        Body::None => req.set("Content-Length", "0").call(),
        Body::Json(v) => req.set("Content-Type", "application/json").send_string(&v.to_string()),
        Body::Form(f) => req.send_form(f),
    };
    let (status, resp) = match r {
        Ok(r) => (r.status(), r),
        Err(ureq::Error::Status(c, r)) => (c, r),
        Err(e) => return Err(format!("{e}")),
    };
    let retry_after = resp.header("Retry-After").and_then(|v| v.trim().parse().ok());
    let mut b = Vec::new();
    let _ = resp.into_reader().take(1 << 20).read_to_end(&mut b);
    Ok(Resp { status, body: String::from_utf8_lossy(&b).into_owned(), retry_after })
}

fn agent() -> ureq::Agent { ureq::AgentBuilder::new().timeout(TIMEOUT).redirects(0).build() }

fn token_reply(r: Resp, old_refresh: &str) -> Result<SpotifyAuth, Fail> {
    if r.status == 429 { return Err(Fail::Limited(r.retry_after.unwrap_or(5))); }
    if r.status == 400 || r.status == 401 { return Err(Fail::Auth); }
    if r.status != 200 { return Err(Fail::Other(format!("Spotify's login answered {}", r.status))); }
    let v: serde_json::Value = serde_json::from_str(&r.body).map_err(|_| Fail::Other("Spotify's login answer was not readable".into()))?;
    let access = v["access_token"].as_str().unwrap_or("").to_string();
    if access.is_empty() { return Err(Fail::Other("Spotify's login gave no token".into())); }
    let refresh = v["refresh_token"].as_str().filter(|s| !s.is_empty()).unwrap_or(old_refresh).to_string();
    let ttl = v["expires_in"].as_u64().unwrap_or(3600).clamp(60, 86_400);
    Ok(SpotifyAuth { access, refresh, expires_at: now_ms() / 1000 + ttl })
}

/// Trade the code from the browser's return for tokens (the PKCE verifier stands in for a client secret).
pub fn exchange_code(ep: &Endpoints, client_id: &str, code: &str, redirect: &str, verifier: &str) -> Result<SpotifyAuth, Fail> {
    let r = send(&agent(), "POST", &format!("{}/api/token", ep.accounts), None,
        Body::Form(&[("grant_type", "authorization_code"), ("code", code), ("redirect_uri", redirect), ("client_id", client_id), ("code_verifier", verifier)])).map_err(Fail::Other)?;
    token_reply(r, "")
}
pub fn refresh(ep: &Endpoints, client_id: &str, refresh_token: &str) -> Result<SpotifyAuth, Fail> {
    let r = send(&agent(), "POST", &format!("{}/api/token", ep.accounts), None,
        Body::Form(&[("grant_type", "refresh_token"), ("refresh_token", refresh_token), ("client_id", client_id)])).map_err(Fail::Other)?;
    token_reply(r, refresh_token)
}

// ---------------------------------------------------------------------------------------------------------------- targets
/// What the person's own player is doing now.
#[derive(Debug, Clone, PartialEq)]
pub struct Local { pub id: Option<String>, pub playing: bool, pub pos_ms: u64 }

/// Somewhere the song can be played: the person's rune, or their Spotify account.
pub trait Backend: Send {
    fn label(&self) -> String;
    fn play(&mut self, id: &str, pos_ms: u64, paused: bool) -> Result<(), Fail>;
    fn pause(&mut self) -> Result<(), Fail>;
    fn seek(&mut self, pos_ms: u64) -> Result<(), Fail>;
    fn local(&mut self) -> Result<Local, Fail>;
    /// Tokens that changed during a call (a refresh), for the caller to save.
    fn auth(&self) -> Option<SpotifyAuth> { None }
}

pub struct RuneTarget { pub base: String, pub zone: String, pub name: String }
impl Backend for RuneTarget {
    fn label(&self) -> String { format!("rune ({})", self.name) }
    fn play(&mut self, id: &str, pos_ms: u64, paused: bool) -> Result<(), Fail> {
        crate::rune::command(&self.base, &self.zone, &serde_json::json!({"type": "play", "trackIds": [format!("sp:{id}")], "startIndex": 0, "positionMs": pos_ms, "paused": paused})).map_err(Fail::Other)
    }
    fn pause(&mut self) -> Result<(), Fail> { crate::rune::command(&self.base, &self.zone, &serde_json::json!({"type": "pause"})).map_err(Fail::Other) }
    fn seek(&mut self, pos_ms: u64) -> Result<(), Fail> { crate::rune::command(&self.base, &self.zone, &serde_json::json!({"type": "seek", "positionMs": pos_ms})).map_err(Fail::Other) }
    fn local(&mut self) -> Result<Local, Fail> {
        let z = crate::rune::zone_now(&self.base, &self.zone, now_ms()).map_err(Fail::Other)?;
        Ok(Local { id: z.id, playing: z.playing, pos_ms: z.pos_ms })
    }
}

pub struct SpotifyTarget { pub ep: Endpoints, pub client_id: String, pub auth: SpotifyAuth, agent: ureq::Agent, changed: bool }
impl SpotifyTarget {
    pub fn new(ep: Endpoints, client_id: String, auth: SpotifyAuth) -> SpotifyTarget { SpotifyTarget { ep, client_id, auth, agent: agent(), changed: false } }

    fn fresh(&mut self) -> Result<(), Fail> {
        if self.auth.expires_at > now_ms() / 1000 + 60 { return Ok(()); }
        self.renew()
    }
    fn renew(&mut self) -> Result<(), Fail> {
        self.auth = refresh(&self.ep, &self.client_id, &self.auth.refresh)?;
        self.changed = true;
        Ok(())
    }
    /// One API call. A 401 refreshes the token once and tries again; the status codes become `Fail`s.
    fn call(&mut self, method: &str, path: &str, body: Option<&serde_json::Value>) -> Result<Resp, Fail> {
        self.fresh()?;
        let url = format!("{}{}", self.ep.api, path);
        let mut renewed = false;
        loop {
            let b = match body { Some(v) => Body::Json(v), None => Body::None };
            let r = send(&self.agent, method, &url, Some(&self.auth.access), b).map_err(Fail::Other)?;
            match r.status {
                200..=299 => return Ok(r),
                401 if !renewed => { renewed = true; self.renew()?; }
                401 => return Err(Fail::Auth),
                429 => return Err(Fail::Limited(r.retry_after.unwrap_or(5).clamp(1, 600))),
                403 => return Err(Fail::Premium),
                404 if r.body.contains("NO_ACTIVE_DEVICE") || r.body.contains("No active device") => return Err(Fail::NoDevice),
                c => return Err(Fail::Other(format!("Spotify answered {c}"))),
            }
        }
    }
    fn play_on(&mut self, id: &str, pos_ms: u64, device: Option<&str>) -> Result<(), Fail> {
        let path = match device { Some(d) => format!("/v1/me/player/play?device_id={}", enc(d)), None => "/v1/me/player/play".into() };
        self.call("PUT", &path, Some(&serde_json::json!({"uris": [format!("spotify:track:{id}")], "position_ms": pos_ms}))).map(|_| ())
    }
    /// A device this account could play on, when none is active: the first one that accepts commands.
    fn pick_device(&mut self) -> Result<Option<String>, Fail> {
        let r = self.call("GET", "/v1/me/player/devices", None)?;
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap_or_default();
        Ok(v["devices"].as_array().and_then(|d| d.iter().find(|d| !d["is_restricted"].as_bool().unwrap_or(false) && d["id"].is_string())).and_then(|d| d["id"].as_str()).map(String::from))
    }
}
impl Backend for SpotifyTarget {
    fn label(&self) -> String { "Spotify".into() }
    fn play(&mut self, id: &str, pos_ms: u64, paused: bool) -> Result<(), Fail> {
        match self.play_on(id, pos_ms, None) {
            Err(Fail::NoDevice) => {
                // Nothing is active, but Spotify may be open somewhere: starting on that device wakes it.
                let Some(d) = self.pick_device()? else { return Err(Fail::NoDevice) };
                self.play_on(id, pos_ms, Some(&d))?;
            }
            r => r?,
        }
        if paused { self.pause()?; }
        Ok(())
    }
    fn pause(&mut self) -> Result<(), Fail> { self.call("PUT", "/v1/me/player/pause", None).map(|_| ()) }
    fn seek(&mut self, pos_ms: u64) -> Result<(), Fail> { self.call("PUT", &format!("/v1/me/player/seek?position_ms={pos_ms}"), None).map(|_| ()) }
    fn local(&mut self) -> Result<Local, Fail> {
        let r = self.call("GET", "/v1/me/player", None)?;
        if r.status == 204 || r.body.trim().is_empty() { return Ok(Local { id: None, playing: false, pos_ms: 0 }); }
        let v: serde_json::Value = serde_json::from_str(&r.body).map_err(|_| Fail::Other("Spotify's answer was not readable".into()))?;
        let id = v["item"]["id"].as_str().filter(|i| is_spotify_id(i)).map(String::from);
        Ok(Local { id, playing: v["is_playing"].as_bool().unwrap_or(false), pos_ms: v["progress_ms"].as_u64().unwrap_or(0) })
    }
    fn auth(&self) -> Option<SpotifyAuth> { self.changed.then(|| self.auth.clone()) }
}

// ---------------------------------------------------------------------------------------------------------------- deciding
/// The friend's last report and when it reached us (our clock).
#[derive(Debug, Clone, PartialEq)]
pub struct Sample { pub play: SpotifyPlay, pub rx_ms: u64 }

/// Where the friend is now. Clocks differ, so only differences count: the position they reported, plus the time since the
/// report reached us on OUR clock. If their clock and ours look nearly aligned the report's own delay is added too.
pub fn expected_pos(s: &Sample, now: u64) -> u64 {
    if s.play.paused { return s.play.pos_ms; }
    let delay = s.rx_ms.saturating_sub(s.play.at_ms);
    let delay = if delay <= MAX_LATENCY_MS { delay } else { 0 };
    s.play.pos_ms + delay + now.saturating_sub(s.rx_ms)
}

/// What we last told the player to do.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied { pub id: String, pub paused: bool, pub pos_ms: u64, pub at: u64, pub sample_rx: u64 }
impl Applied { fn predicted(&self, now: u64) -> u64 { if self.paused { self.pos_ms } else { self.pos_ms + now.saturating_sub(self.at) } } }

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mem {
    pub applied: Option<Applied>,
    /// The last time the person's player was asked about (or told something).
    pub last_check: u64,
    /// Nothing is sent before this (rate limits, failures).
    pub hold_until: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Nothing,
    /// A new report that agrees with what is already playing: just remember it.
    Absorb,
    /// Read the person's player to look for drift.
    Check,
    Play { id: String, pos_ms: u64, paused: bool },
    Pause,
    Seek(u64),
    /// The person started something else; stop following.
    Ended(&'static str),
}

/// The whole policy, with no I/O. `local` is the person's own player, only once a `Check` asked for it.
pub fn decide(sharer: &Sample, mem: &Mem, local: Option<&Local>, now: u64) -> Action {
    if now < mem.hold_until { return Action::Nothing; }
    let want = expected_pos(sharer, now);
    let (id, paused) = (&sharer.play.id, sharer.play.paused);
    let Some(a) = &mem.applied else { return Action::Play { id: id.clone(), pos_ms: want, paused } };
    if a.id != *id { return Action::Play { id: id.clone(), pos_ms: want, paused }; }
    if a.paused != paused { return if paused { Action::Pause } else { Action::Play { id: id.clone(), pos_ms: want, paused: false } }; }
    if a.sample_rx != sharer.rx_ms {
        // A fresh report: steady playing explains it (a renewal), or the friend jumped.
        return if want.abs_diff(a.predicted(now)) > SEEK_SLACK_MS { Action::Seek(want) } else { Action::Absorb };
    }
    let Some(l) = local else { return if now < mem.last_check + CHECK_MS { Action::Nothing } else { Action::Check } };
    if let Some(other) = &l.id { if other != id { return Action::Ended("You started another song, so you are no longer listening along."); } }
    if paused { return if l.playing && l.id.is_some() { Action::Pause } else { Action::Nothing }; }
    // They paused it themselves: that is theirs to undo. Otherwise keep up.
    if l.id.is_some() && l.playing && l.pos_ms.abs_diff(want) > DRIFT_MS { return Action::Seek(want); }
    Action::Nothing
}

/// What the person should be told.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Outcome {
    pub ended: bool,
    pub message: String,
    /// "device" (open Spotify somewhere), "login" (log in again), "premium" (needs Premium), or empty.
    pub needs: String,
}

fn fail_outcome(f: &Fail, mem: &mut Mem, now: u64) -> Outcome {
    match f {
        Fail::NoDevice => { mem.hold_until = now + CHECK_MS; Outcome { ended: false, message: "Open Spotify on a device first. This will start by itself.".into(), needs: "device".into() } }
        Fail::Premium => Outcome { ended: true, message: "Spotify would not let mega control playback. That needs Spotify Premium.".into(), needs: "premium".into() },
        Fail::Limited(s) => { mem.hold_until = now + s * 1000 + 500; Outcome { ended: false, message: format!("Spotify asked for a pause; trying again in {s} seconds."), needs: String::new() } }
        Fail::Auth => Outcome { ended: true, message: "Spotify no longer accepts the login on this device. Log in again.".into(), needs: "login".into() },
        Fail::Other(m) => { mem.hold_until = now + CHECK_MS; Outcome { ended: false, message: format!("Could not reach the player: {m}"), needs: String::new() } }
    }
}

/// One look at the friend's report and the player: do what `decide` says, and say how it went. At most one write per
/// change, one read per 15 seconds.
pub fn step(mem: &mut Mem, be: &mut dyn Backend, sharer: &Sample, now: u64) -> Outcome {
    let mut act = decide(sharer, mem, None, now);
    if act == Action::Check {
        mem.last_check = now;
        match be.local() {
            Ok(l) => act = decide(sharer, mem, Some(&l), now),
            Err(f) => return fail_outcome(&f, mem, now),
        }
    }
    let want = expected_pos(sharer, now);
    let id = sharer.play.id.clone();
    let done = |mem: &mut Mem, paused: bool, pos_ms: u64| {
        mem.applied = Some(Applied { id: id.clone(), paused, pos_ms, at: now, sample_rx: sharer.rx_ms });
        mem.last_check = now;
    };
    let res = match act {
        Action::Nothing => return Outcome::default(),
        Action::Absorb => { done(mem, sharer.play.paused, want); return Outcome::default(); }
        Action::Check => return Outcome::default(),
        Action::Ended(why) => return Outcome { ended: true, message: why.into(), needs: String::new() },
        Action::Play { id: i, pos_ms, paused } => be.play(&i, pos_ms, paused).map(|_| (paused, pos_ms)),
        Action::Pause => be.pause().map(|_| (true, mem.applied.as_ref().map_or(want, |a| a.predicted(now)))),
        Action::Seek(p) => be.seek(p).map(|_| (sharer.play.paused, p)),
    };
    match res {
        Ok((paused, pos)) => { done(mem, paused, pos); Outcome::default() }
        Err(f) => fail_outcome(&f, mem, now),
    }
}

/// "Open in Spotify" for a track.
pub fn open_link(id: &str) -> Option<String> { is_spotify_id(id).then(|| format!("https://open.spotify.com/track/{id}")) }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{Envelope, NowPlaying};

    const ID: &str = "4uLU6hMCjMI75M1A2tKUQC";
    fn play(pos: u64, at: u64, paused: bool) -> SpotifyPlay { SpotifyPlay { id: ID.into(), pos_ms: pos, at_ms: at, paused } }
    fn sample(pos: u64, at: u64, rx: u64, paused: bool) -> Sample { Sample { play: play(pos, at, paused), rx_ms: rx } }

    #[test]
    fn pkce_matches_the_rfc_example_and_verifiers_are_fresh() {
        assert_eq!(challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let (a, b) = (new_verifier(), new_verifier());
        assert_ne!(a, b);
        assert!((43..=128).contains(&a.len()) && a.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'), "{a}");
        assert_ne!(new_state(), new_state());
    }
    #[test]
    fn the_login_address_has_what_spotify_needs_and_no_secret() {
        let ep = Endpoints { accounts: "https://accounts.spotify.com".into(), api: String::new() };
        let u = auth_url(&ep, "abc123", "http://127.0.0.1:7420/spotify/callback", "CHAL", "ST");
        for part in ["client_id=abc123", "response_type=code", "code_challenge_method=S256", "code_challenge=CHAL", "state=ST",
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A7420%2Fspotify%2Fcallback", "scope=user-modify-playback-state%20user-read-playback-state"] { assert!(u.contains(part), "{part} in {u}"); }
        assert!(!u.contains("secret") && !u.contains("verifier"));
        assert!(crate::api::link_ok(&u), "the page can hand it to the system browser");
    }
    #[test]
    fn only_this_apps_own_loopback_address_is_a_redirect() {
        assert!(redirect_ok("http://127.0.0.1:7420/spotify/callback"));
        for bad in ["http://localhost:7420/spotify/callback", "https://127.0.0.1:7420/spotify/callback", "http://127.0.0.1/spotify/callback", "http://127.0.0.1:7420/other", "http://127.0.0.1:7420/spotify/callback?x=1", "http://evil.example/spotify/callback", "http://127.0.0.1:0/spotify/callback", "http://127.0.0.1:99999/spotify/callback"] { assert!(!redirect_ok(bad), "{bad}"); }
    }
    #[test]
    fn a_query_is_read_back() {
        let q = parse_query("code=a%2Bb%3D&state=x+y&error=access_denied");
        assert_eq!((q["code"].as_str(), q["state"].as_str(), q["error"].as_str()), ("a+b=", "x y", "access_denied"));
        assert!(parse_query("code=%").contains_key("code"), "a stray % does not panic");
    }
    #[test]
    fn a_client_id_is_only_letters_and_numbers() {
        assert_eq!(client_id(" 0123abcDEF "), "0123abcDEF");
        assert_eq!(client_id("bad id!"), "");
    }

    #[test]
    fn the_friends_position_is_extrapolated_on_our_own_clock() {
        // reported 60 s in; it reached us at local 1_000_500; now it is local 1_010_500: 10 s later
        let s = sample(60_000, 1_000_000, 1_000_500, false);
        assert_eq!(expected_pos(&s, 1_010_500), 60_000 + 500 + 10_000, "clocks that look aligned: the report's delay counts");
        // a clock an hour off: only the time since it reached us counts
        let off = sample(60_000, 10_000_000 - 3_600_000, 10_000_500, false);
        assert_eq!(expected_pos(&off, 10_010_500), 70_000);
        let ahead = sample(60_000, 10_000_000 + 3_600_000, 10_000_500, false);
        assert_eq!(expected_pos(&ahead, 10_010_500), 70_000, "a clock ahead does not run time backwards");
        assert_eq!(expected_pos(&sample(60_000, 1, 2, true), 99_999_999), 60_000, "paused stays put");
    }

    fn fresh_mem(s: &Sample, now: u64) -> Mem {
        Mem { applied: Some(Applied { id: s.play.id.clone(), paused: s.play.paused, pos_ms: expected_pos(s, now), at: now, sample_rx: s.rx_ms }), last_check: now, hold_until: 0 }
    }
    #[test]
    fn deciding_follows_the_friend() {
        let t0 = 1_000_000;
        let s = sample(10_000, t0, t0, false);
        // nothing applied yet: start the song where the friend is
        assert_eq!(decide(&s, &Mem::default(), None, t0 + 1000), Action::Play { id: ID.into(), pos_ms: 11_000, paused: false });
        let m = fresh_mem(&s, t0);
        assert_eq!(decide(&s, &m, None, t0 + 5_000), Action::Nothing, "no call needed while all is well");
        assert_eq!(decide(&s, &m, None, t0 + CHECK_MS), Action::Check, "a drift check only every 15 s");
        // a new song
        let mut other = s.clone(); other.play.id = "0VjIjW4GlUZAMYd2vXMi3b".into(); other.rx_ms += 5000;
        assert!(matches!(decide(&other, &m, None, t0 + 6_000), Action::Play { ref id, .. } if id == "0VjIjW4GlUZAMYd2vXMi3b"));
        // pause, then resume at the new place
        let paused = sample(15_000, t0 + 5000, t0 + 5000, true);
        assert_eq!(decide(&paused, &m, None, t0 + 5_100), Action::Pause);
        let mp = fresh_mem(&paused, t0 + 5_100);
        let resumed = sample(15_000, t0 + 9000, t0 + 9000, false);
        assert_eq!(decide(&resumed, &mp, None, t0 + 9_100), Action::Play { id: ID.into(), pos_ms: 15_100, paused: false });
        // a renewal that agrees with steady playing is only remembered; a jump is a seek
        let renewal = sample(10_000 + 300_000, t0 + 300_000, t0 + 300_000, false);
        assert_eq!(decide(&renewal, &m, None, t0 + 300_050), Action::Absorb);
        let jump = sample(120_000, t0 + 20_000, t0 + 20_000, false);
        assert_eq!(decide(&jump, &m, None, t0 + 20_000), Action::Seek(120_000));
    }
    #[test]
    fn deciding_corrects_drift_but_respects_the_person() {
        let t0 = 1_000_000;
        let s = sample(10_000, t0, t0, false);
        let m = fresh_mem(&s, t0);
        let now = t0 + CHECK_MS;
        let want = 10_000 + CHECK_MS;
        let local = |id: Option<&str>, playing, pos| Local { id: id.map(String::from), playing, pos_ms: pos };
        assert_eq!(decide(&s, &m, Some(&local(Some(ID), true, want + 1_500)), now), Action::Nothing, "within 2 s");
        assert_eq!(decide(&s, &m, Some(&local(Some(ID), true, want + 4_000)), now), Action::Seek(want));
        assert_eq!(decide(&s, &m, Some(&local(Some(ID), true, want - 9_000)), now), Action::Seek(want));
        assert_eq!(decide(&s, &m, Some(&local(Some(ID), false, 0)), now), Action::Nothing, "they paused it themselves");
        assert_eq!(decide(&s, &m, Some(&local(None, false, 0)), now), Action::Nothing);
        assert!(matches!(decide(&s, &m, Some(&local(Some("0VjIjW4GlUZAMYd2vXMi3b"), true, 0)), now), Action::Ended(_)), "another song ends it");
        let ps = sample(30_000, t0, t0, true);
        assert_eq!(decide(&ps, &fresh_mem(&ps, t0), Some(&local(Some(ID), true, 31_000)), now), Action::Pause);
        // a hold (rate limit) sends nothing at all
        let mut held = Mem::default(); held.hold_until = now + 10;
        assert_eq!(decide(&s, &held, None, now), Action::Nothing);
    }

    #[test]
    fn a_spotify_status_is_news_only_when_it_changes() {
        let t0 = 5_000_000;
        let mk = |p: SpotifyPlay| NowPlaying { title: "T".into(), artist: "A".into(), spotify: Some(p) };
        let sent = mk(play(10_000, t0, false));
        assert!(!mk(play(14_000, t0 + 4_000, false)).is_news_after(&sent), "plain progress");
        assert!(mk(play(40_000, t0 + 4_000, false)).is_news_after(&sent), "a seek forward");
        assert!(mk(play(2_000, t0 + 4_000, false)).is_news_after(&sent), "a seek back");
        assert!(mk(play(14_000, t0 + 4_000, true)).is_news_after(&sent), "a pause");
        let mut next = play(0, t0 + 4_000, false); next.id = "0VjIjW4GlUZAMYd2vXMi3b".into();
        assert!(mk(next).is_news_after(&sent), "a new song");
        assert!(NowPlaying::text("T", "A").is_news_after(&sent), "switching to a local file");
    }

    /// What an older Megatron (0.3.5, 0.3.6) parses: the status without the new field, and no `deny_unknown_fields` anywhere.
    #[derive(serde::Deserialize, Debug, PartialEq)]
    struct OldNowPlaying { title: String, #[serde(default)] artist: String }
    #[derive(serde::Deserialize, Debug)]
    #[serde(tag = "t")]
    enum OldEnvelope { Profile { name: String, ts: u64, #[serde(default)] listen: Option<OldNowPlaying>, #[serde(default)] direct: bool, #[serde(default)] direct_sealed: bool }, #[serde(other)] Unknown }

    #[test]
    fn the_new_status_reads_as_a_plain_one_to_an_older_app_and_back() {
        let np = NowPlaying { title: "Song".into(), artist: "Band".into(), spotify: Some(play(61_000, 1_700_000_000_000, true)) };
        let env = Envelope::Profile { name: "Ana".into(), ts: 5, listen: Some(np.clone()), avatar: None, avatar_cleared: false, direct: true, direct_sealed: true };
        let bytes = serde_json::to_vec(&env).unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("\"spotify\""));
        match serde_json::from_slice::<OldEnvelope>(&bytes).unwrap() {
            OldEnvelope::Profile { name, listen, .. } => { assert_eq!(name, "Ana"); assert_eq!(listen, Some(OldNowPlaying { title: "Song".into(), artist: "Band".into() })); }
            OldEnvelope::Unknown => panic!("an older app must still see a profile, not an unknown message"),
        }
        // the current code accepts it, validation included, and an older sender's plain status still parses
        assert!(matches!(Envelope::parse(&bytes).unwrap(), Envelope::Profile { listen: Some(ref l), .. } if *l == np));
        let plain = br#"{"t":"Profile","name":"Ana","ts":5,"listen":{"title":"Song","artist":"Band"}}"#;
        assert!(matches!(Envelope::parse(plain).unwrap(), Envelope::Profile { listen: Some(ref l), .. } if l.spotify.is_none()));
        // a plain status serialises exactly as before: no new field appears for a local song
        let local = Envelope::Profile { name: "Ana".into(), ts: 5, listen: Some(NowPlaying::text("Song", "Band")), avatar: None, avatar_cleared: false, direct: false, direct_sealed: false };
        assert!(!String::from_utf8(serde_json::to_vec(&local).unwrap()).unwrap().contains("spotify"));
        // a bad track id is refused by validation rather than passed on to a player
        let bad = String::from_utf8(bytes.clone()).unwrap().replace(ID, "not an id");
        assert!(Envelope::parse(bad.as_bytes()).is_err());
        // the envelope stays far inside the size limit
        assert!(bytes.len() < 1024);
    }
}
