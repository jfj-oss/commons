//! What rune (the local music player) is playing, read from its own HTTP API.
//!
//! This lives in the core, not the page, so the browser never has to reach rune itself: no CORS, no CSP hole, and
//! nothing leaves this machine except the two lines the person chose to share. rune has no account or token of its
//! own for this API; it only listens on loopback unless the person set it up otherwise.
use crate::envelope::{NowPlaying, MAX_LISTEN_LINE};
use std::io::Read;
use std::time::Duration;

pub const DEFAULT_URL: &str = "http://127.0.0.1:4545";
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
struct Zone { id: String, #[serde(default)] kind: String }
#[derive(serde::Deserialize)]
struct PlayerState {
    status: String,
    #[serde(default)] queue: Vec<QueueItem>,
    #[serde(default)] index: i64,
    #[serde(default, rename = "nowPlaying")] now_playing: Option<Radio>,
}
#[derive(serde::Deserialize)]
struct QueueItem { track: Track }
#[derive(serde::Deserialize)]
struct Track { #[serde(default)] title: String, #[serde(default)] artists: Vec<Artist> }
#[derive(serde::Deserialize)]
struct Artist { #[serde(default)] name: String }
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

/// The song in a player state, if one is actually playing.
fn playing(state: &PlayerState) -> Option<NowPlaying> {
    if state.status != "playing" { return None; }
    // A track in the queue is the usual case (local files and Spotify alike).
    if let Some(item) = usize::try_from(state.index).ok().and_then(|i| state.queue.get(i)) {
        let title = tidy(&item.track.title);
        if !title.is_empty() {
            let artist = tidy(&item.track.artists.iter().map(|a| a.name.as_str()).filter(|n| !n.trim().is_empty()).collect::<Vec<_>>().join(", "));
            return Some(NowPlaying { title, artist });
        }
    }
    // Live radio has no track: rune reports the stream's own metadata instead.
    let r = state.now_playing.as_ref()?;
    let title = tidy(r.title.as_deref().unwrap_or(""));
    if title.is_empty() { return None; }
    let artist = tidy(r.artist.as_deref().or(r.station.as_deref()).unwrap_or(""));
    Some(NowPlaying { title, artist })
}

/// Ask rune for the zone that is playing, and what it is playing. `Ok(None)` means nothing is playing (or rune is
/// there and idle); `Err` means rune could not be read at all, which is not the same as "stopped" and is left alone.
pub fn now_playing(base: &str) -> Result<Option<NowPlaying>, String> {
    let base = clean_url(base)?;
    let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).build();
    let zones: Vec<Zone> = get(&agent, &format!("{base}/api/zones"))?;
    let mut playing_zone: Option<PlayerState> = None;
    for z in zones {
        if z.id.is_empty() { continue; }
        let Ok(state) = get::<PlayerState>(&agent, &format!("{base}/api/player/{}", segment(&z.id))) else { continue };
        if state.status == "playing" {
            let pc = z.kind == "pc";
            if playing_zone.is_none() || pc { playing_zone = Some(state); }
            if pc { break; } // the PC speakers are the local choice when several zones happen to play
        }
    }
    Ok(playing_zone.as_ref().and_then(playing))
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
        assert_eq!(playing(&s), Some(NowPlaying { title: "Detective Arriving on the Scene".into(), artist: "Sea Power".into() }));
        assert_eq!(playing(&state(r#"{"status":"paused","index":0,"queue":[{"track":{"title":"x","artists":[]}}]}"#)), None);
        assert_eq!(playing(&state(r#"{"status":"stopped","index":-1,"queue":[]}"#)), None);
        let dirty = state("{\"status\":\"playing\",\"index\":0,\"queue\":[{\"track\":{\"title\":\"two\\nlines\\u0000\",\"artists\":[{\"name\":\"A\"},{\"name\":\"B\"}]}}]}");
        assert_eq!(playing(&dirty), Some(NowPlaying { title: "two lines".into(), artist: "A, B".into() }));
        // radio: no queue, metadata from the stream
        assert_eq!(playing(&state(r#"{"status":"playing","index":-1,"queue":[],"nowPlaying":{"title":"Evening Jazz","station":"WXYZ"}}"#)), Some(NowPlaying { title: "Evening Jazz".into(), artist: "WXYZ".into() }));
    }
    #[test]
    fn a_title_is_never_longer_than_the_field_allows() {
        let long = "é".repeat(MAX_LISTEN_LINE);
        let s = state(&serde_json::json!({"status":"playing","index":0,"queue":[{"track":{"title":long,"artists":[]}}]}).to_string());
        let got = playing(&s).unwrap();
        assert!(got.title.len() <= MAX_LISTEN_LINE && got.title.is_char_boundary(got.title.len()));
    }
}
