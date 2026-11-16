//! What rune (the local music player) is playing, read from its own HTTP API.
//!
//! This lives in the core, not the page, so the browser never has to reach rune itself: no CORS, no CSP hole, and
//! nothing leaves this machine except the two lines the person chose to share. rune has no account or token of its
//! own for this API; it only listens on loopback unless the person set it up otherwise.
use crate::envelope::{is_spotify_id, NowPlaying, SpotifyPlay, MAX_LISTEN_LINE};
use std::io::Read;
use std::time::Duration;

pub const DEFAULT_URL: &str = "http://127.0.0.1:4545";
/// Where rune is looked for until the person says otherwise. `COMMONS_RUNE` moves it, so that a test's apps (which share by
/// default like everyone's) never ask the rune of the person running the test; so does `COMMONS_FAST_KDF`, see below.
pub fn default_url() -> String {
    if let Some(u) = std::env::var("COMMONS_RUNE").ok().and_then(|u| clean_url(&u).ok()) { return u; }
    // Every test bridge runs with COMMONS_FAST_KDF (never set for a person's app): those look where nothing answers.
    if std::env::var_os("COMMONS_FAST_KDF").is_some() { return NOWHERE_URL.to_string(); }
    DEFAULT_URL.to_string()
}
/// An address no rune answers on (a closed loopback port).
pub const NOWHERE_URL: &str = "http://127.0.0.1:1";
const TIMEOUT: Duration = Duration::from_secs(2);
const MAX_BODY: u64 = 4 << 20;

/// A rune address we are willing to call: http(s), a host, no credentials or query string.
pub fn clean_url(u: &str) -> Result<String, String> {
    let u = u.trim().trim_end_matches('/');
    let bad = || format!("\"{u}\" is not a rune address. It should look like {DEFAULT_URL}");
    let rest = u.strip_prefix("https://").or_else(|| u.strip_prefix("http://")).ok_or_else(bad)?;
    let host = rest.split('/').next().unwrap_or("");
    let name = host.rsplit_once(':').filter(|(_, p)| p.parse::<u16>().is_ok()).map(|(h, _)| h).unwrap_or(host);
    let ok = !name.is_empty() && !name.contains(':') && name.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_')) && !rest.contains(['?', '#', '@', ' ', '\\']);
    if ok { Ok(u.to_string()) } else { Err(bad()) }
}

#[derive(serde::Deserialize)]
struct Zone { id: String, #[serde(default)] kind: String, #[serde(default)] name: String }
#[derive(serde::Deserialize)]
struct PlayerState {
    status: String,
    #[serde(default)] queue: Vec<QueueItem>,
    #[serde(default)] index: i64,
    #[serde(default, rename = "nowPlaying")] now_playing: Option<Radio>,
    #[serde(default)] hold: Option<Hold>,
    #[serde(default, rename = "positionMs")] position_ms: f64,
    #[serde(default, rename = "updatedAt")] updated_at: f64,
}
#[derive(serde::Deserialize)]
struct QueueItem { track: Track }
#[derive(serde::Deserialize)]
struct Track {
    #[serde(default)] id: String, #[serde(default)] title: String, #[serde(default)] artists: Vec<Artist>,
    #[serde(default)] source: String, #[serde(default, rename = "spotifyUri")] spotify_uri: Option<String>,
}
impl Track {
    /// The Spotify track id, for a Spotify song (rune names those `sp:<id>`, and keeps the uri beside it).
    fn spotify_id(&self) -> Option<String> {
        if self.source != "spotify" && !self.id.starts_with("sp:") { return None; }
        let id = self.spotify_uri.as_deref().and_then(|u| u.strip_prefix("spotify:track:")).or_else(|| self.id.strip_prefix("sp:"))?;
        is_spotify_id(id).then(|| id.to_string())
    }
}
#[derive(serde::Deserialize)]
struct Artist { #[serde(default)] name: String }
#[derive(serde::Deserialize)]
struct Hold { #[serde(default)] message: String }
#[derive(serde::Deserialize)]
struct Radio { #[serde(default)] title: Option<String>, #[serde(default)] artist: Option<String>, #[serde(default)] station: Option<String> }

fn get<T: serde::de::DeserializeOwned>(agent: &ureq::Agent, url: &str) -> Result<T, String> {
    let resp = agent.get(url).call().map_err(|e| format!("{e}"))?;
    let mut body = Vec::new();
    resp.into_reader().take(MAX_BODY).read_to_end(&mut body).map_err(|e| format!("{e}"))?;
    serde_json::from_slice(&body).map_err(|e| format!("{e}"))
}

/// One line, no controls, no huge titles: this text ends up on other people's screens.
fn tidy(s: &str) -> String {
    let s: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.len() <= MAX_LISTEN_LINE { return s; }
    let mut end = MAX_LISTEN_LINE;
    while !s.is_char_boundary(end) { end -= 1; }
    s[..end].trim_end().to_string()
}

/// Where the song is now, as rune reports it: its position as of `updatedAt`, carried forward while it plays. A stale or
/// foreign clock is not trusted for long (the carry-forward is capped), and the sample is stamped with our own clock.
fn position_now(state: &PlayerState, now_ms: u64) -> u64 {
    let base = if state.position_ms.is_finite() && state.position_ms > 0.0 { state.position_ms as u64 } else { 0 };
    if state.status != "playing" || !state.updated_at.is_finite() || state.updated_at <= 0.0 { return base; }
    base + now_ms.saturating_sub(state.updated_at as u64).min(30_000)
}

/// The song in a player state, if one is actually playing (or, for a Spotify song, paused: friends listening along need
/// to hear about the pause).
fn playing(state: &PlayerState, now_ms: u64) -> Option<NowPlaying> {
    let paused = state.status == "paused";
    if state.status != "playing" && !paused { return None; }
    // A track in the queue is the usual case (local files and Spotify alike).
    if let Some(item) = usize::try_from(state.index).ok().and_then(|i| state.queue.get(i)) {
        let title = tidy(&item.track.title);
        let spotify = item.track.spotify_id().map(|id| SpotifyPlay { id, pos_ms: position_now(state, now_ms), at_ms: now_ms, paused });
        if paused && spotify.is_none() { return None; }
        if !title.is_empty() {
            let artist = tidy(&item.track.artists.iter().map(|a| a.name.as_str()).filter(|n| !n.trim().is_empty()).collect::<Vec<_>>().join(", "));
            return Some(NowPlaying { title, artist, spotify });
        }
        // A Spotify song rune never stored has no title yet: friends can still follow it by id, but have nothing to read.
    }
    if paused { return None; }
    // Live radio has no track: rune reports the stream's own metadata instead.
    let r = state.now_playing.as_ref()?;
    let title = tidy(r.title.as_deref().unwrap_or(""));
    if title.is_empty() { return None; }
    let artist = tidy(r.artist.as_deref().or(r.station.as_deref()).unwrap_or(""));
    Some(NowPlaying { title, artist, spotify: None })
}

/// What a zone is doing, for listening along: which Spotify song (if any) and where, as of now.
pub struct ZoneNow { pub id: Option<String>, pub playing: bool, pub pos_ms: u64, pub hold: Option<String> }

pub fn zone_now(base: &str, zone: &str, now_ms: u64) -> Result<ZoneNow, String> {
    let base = clean_url(base)?;
    let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).build();
    let st: PlayerState = get(&agent, &format!("{base}/api/player/{}", segment(zone)))?;
    let id = usize::try_from(st.index).ok().and_then(|i| st.queue.get(i)).and_then(|q| q.track.spotify_id());
    Ok(ZoneNow { id, playing: st.status == "playing" || st.status == "loading", pos_ms: position_now(&st, now_ms), hold: st.hold.map(|h| h.message) })
}

/// The zones rune offers: (id, name). Empty if rune is not there.
pub fn zones(base: &str) -> Result<Vec<(String, String)>, String> {
    let base = clean_url(base)?;
    let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).build();
    let zs: Vec<Zone> = get(&agent, &format!("{base}/api/zones"))?;
    Ok(zs.into_iter().filter(|z| !z.id.is_empty()).map(|z| { let n = if z.name.is_empty() { z.id.clone() } else { z.name.clone() }; (z.id, n) }).collect())
}

/// One player command for a zone (play, pause, resume, seek).
pub fn command(base: &str, zone: &str, body: &serde_json::Value) -> Result<(), String> {
    let base = clean_url(base)?;
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(5)).build();
    match agent.post(&format!("{base}/api/player/{}", segment(zone))).set("Content-Type", "application/json").send_string(&body.to_string()) {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(c, r)) => Err(format!("rune answered {c}: {}", r.into_string().unwrap_or_default().chars().take(120).collect::<String>())),
        Err(e) => Err(format!("{e}")),
    }
}

/// Ask rune for the zone that is playing, and what it is playing. `Ok(None)` means nothing is playing (or rune is
/// there and idle); `Err` means rune could not be read at all, which is not the same as "stopped" and is left alone.
pub fn now_playing(base: &str) -> Result<Option<NowPlaying>, String> { now_playing_at(base, crate::spotify::now_ms()) }
pub fn now_playing_at(base: &str, now_ms: u64) -> Result<Option<NowPlaying>, String> {
    let base = clean_url(base)?;
    let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).build();
    let zones: Vec<Zone> = get(&agent, &format!("{base}/api/zones"))?;
    let mut playing_zone: Option<PlayerState> = None;
    let mut paused_zone: Option<PlayerState> = None;
    for z in zones {
        if z.id.is_empty() { continue; }
        let Ok(state) = get::<PlayerState>(&agent, &format!("{base}/api/player/{}", segment(&z.id))) else { continue };
        if state.status == "paused" && paused_zone.is_none() { paused_zone = Some(state); continue; }
        if state.status == "playing" {
            let pc = z.kind == "pc";
            if playing_zone.is_none() || pc { playing_zone = Some(state); }
            if pc { break; } // the PC speakers are the local choice when several zones happen to play
        }
    }
    Ok(playing_zone.as_ref().and_then(|z| playing(z, now_ms)).or_else(|| paused_zone.as_ref().and_then(|z| playing(z, now_ms))))
}

/// What rune's own player shows right now, for a bar that controls it (the call bar of the desktop app): the zone that is
/// playing (the PC speakers first), or else one that is paused, with the song's two lines. `Ok(None)` is a rune that is
/// there with nothing in any queue.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Current { pub zone: String, pub zone_name: String, pub playing: bool, pub title: String, pub artist: String }

pub fn current(base: &str) -> Result<Option<Current>, String> {
    let base = clean_url(base)?;
    let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).build();
    let zones: Vec<Zone> = get(&agent, &format!("{base}/api/zones"))?;
    let mut best: Option<(u8, Current)> = None;
    for z in zones {
        if z.id.is_empty() { continue; }
        let Ok(st) = get::<PlayerState>(&agent, &format!("{base}/api/player/{}", segment(&z.id))) else { continue };
        let playing = matches!(st.status.as_str(), "playing" | "loading");
        if !playing && st.status != "paused" { continue; }
        let Some((title, artist)) = line_of(&st) else { continue };
        // the PC speakers while they play, then any playing zone, then a paused one
        let rank = match (playing, z.kind == "pc") { (true, true) => 3, (true, false) => 2, (false, true) => 1, (false, false) => 0 };
        if best.as_ref().map_or(true, |(r, _)| rank > *r) {
            let zone_name = if z.name.is_empty() { z.id.clone() } else { z.name.clone() };
            best = Some((rank, Current { zone: z.id.clone(), zone_name, playing, title, artist }));
        }
    }
    Ok(best.map(|(_, c)| c))
}

/// The title and artist of what a player state has up, whatever the source (a queue entry, or a radio stream's own metadata).
fn line_of(st: &PlayerState) -> Option<(String, String)> {
    if let Some(item) = usize::try_from(st.index).ok().and_then(|i| st.queue.get(i)) {
        let title = tidy(&item.track.title);
        if !title.is_empty() { return Some((title, tidy(&item.track.artists.iter().map(|a| a.name.as_str()).filter(|n| !n.trim().is_empty()).collect::<Vec<_>>().join(", ")))); }
    }
    let r = st.now_playing.as_ref()?;
    let title = tidy(r.title.as_deref().unwrap_or(""));
    if title.is_empty() { return None; }
    Some((title, tidy(r.artist.as_deref().or(r.station.as_deref()).unwrap_or(""))))
}

/// One of the four plain controls of a zone: `pause`, `resume`, `next`, `prev`.
pub fn control(base: &str, zone: &str, what: &str) -> Result<(), String> {
    if !matches!(what, "pause" | "resume" | "next" | "prev") { return Err(format!("unknown control {what}")); }
    command(base, zone, &serde_json::json!({"type": what}))
}

/// A zone id goes into one path segment. Only what needs escaping is escaped, so familiar ids stay readable.
fn segment(s: &str) -> String {
    s.bytes().map(|b| match b {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b':' => (b as char).to_string(),
        _ => format!("%{b:02X}"),
    }).collect()
}

#[cfg(test)]
mod unit {
    use super::*;
    #[test]
    fn addresses_are_the_ones_a_person_meant() {
        assert_eq!(clean_url(" http://127.0.0.1:4545/ ").unwrap(), "http://127.0.0.1:4545");
        assert_eq!(clean_url("http://music.local:4545").unwrap(), "http://music.local:4545");
        for bad in ["", "127.0.0.1:4545", "ftp://x", "http://", "http://a b", "http://u:p@host", "http://x/?q=1", "http://x/#y"] { assert!(clean_url(bad).is_err(), "{bad:?}"); }
    }
    #[test]
    fn zone_ids_stay_in_their_segment() {
        assert_eq!(segment("spotify:abc123"), "spotify:abc123");
        assert_eq!(segment("a/b?c"), "a%2Fb%3Fc");
    }
}

#[cfg(test)]
mod parse_tests {
    use super::*;
    fn state(json: &str) -> PlayerState { serde_json::from_str(json).unwrap() }
    #[test]
    fn only_a_playing_zone_counts_and_text_is_cleaned() {
        let s = state(r#"{"status":"playing","index":0,"queue":[{"track":{"title":"Detective Arriving on the Scene","artists":[{"name":"Sea Power"}]}}]}"#);
        assert_eq!(playing(&s, 1000), Some(NowPlaying::text("Detective Arriving on the Scene", "Sea Power")));
        assert_eq!(playing(&state(r#"{"status":"paused","index":0,"queue":[{"track":{"title":"x","artists":[]}}]}"#), 1000), None);
        assert_eq!(playing(&state(r#"{"status":"stopped","index":-1,"queue":[]}"#), 1000), None);
        let dirty = state("{\"status\":\"playing\",\"index\":0,\"queue\":[{\"track\":{\"title\":\"two\\nlines\\u0000\",\"artists\":[{\"name\":\"A\"},{\"name\":\"B\"}]}}]}");
        assert_eq!(playing(&dirty, 1000), Some(NowPlaying::text("two lines", "A, B")));
        // radio: no queue, metadata from the stream
        assert_eq!(playing(&state(r#"{"status":"playing","index":-1,"queue":[],"nowPlaying":{"title":"Evening Jazz","station":"WXYZ"}}"#), 1000), Some(NowPlaying::text("Evening Jazz", "WXYZ")));
    }
    #[test]
    fn a_title_is_never_longer_than_the_field_allows() {
        let long = "é".repeat(MAX_LISTEN_LINE);
        let s = state(&serde_json::json!({"status":"playing","index":0,"queue":[{"track":{"title":long,"artists":[]}}]}).to_string());
        let got = playing(&s, 1000).unwrap();
        assert!(got.title.len() <= MAX_LISTEN_LINE && got.title.is_char_boundary(got.title.len()));
    }
    #[test]
    fn a_spotify_song_carries_where_it_is_and_a_pause_is_reported() {
        let song = |status: &str| state(&format!(r#"{{"status":"{status}","index":0,"positionMs":61000,"updatedAt":99000,"queue":[{{"track":{{"id":"sp:4uLU6hMCjMI75M1A2tKUQC","source":"spotify","spotifyUri":"spotify:track:4uLU6hMCjMI75M1A2tKUQC","title":"Song","artists":[{{"name":"Band"}}]}}}}]}}"#));
        let p = playing(&song("playing"), 100_000).unwrap();
        let sp = p.spotify.unwrap();
        assert_eq!((sp.id.as_str(), sp.pos_ms, sp.at_ms, sp.paused), ("4uLU6hMCjMI75M1A2tKUQC", 62_000, 100_000, false), "carried forward 1 s from updatedAt");
        let q = playing(&song("paused"), 100_000).unwrap().spotify.unwrap();
        assert!(q.paused && q.pos_ms == 61_000, "a pause does not run on");
        // a local song that is paused is not shared at all, as before
        assert_eq!(playing(&state(r#"{"status":"paused","index":0,"queue":[{"track":{"id":"t1","source":"local","title":"x","artists":[]}}]}"#), 1), None);
        // a bad id is not shared as a Spotify song, but the title still is
        let bad = state(r#"{"status":"playing","index":0,"queue":[{"track":{"id":"sp:nope","source":"spotify","title":"x","artists":[]}}]}"#);
        assert_eq!(playing(&bad, 1).unwrap().spotify, None);
    }
}
