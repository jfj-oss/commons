//! Listening along, against FAKE servers only: a pretend Spotify (accounts + Web API) and a pretend rune. The real ones are
//! never called, so these cannot spend anyone's rate limit or touch anyone's player.
use commons_core::envelope::SpotifyPlay;
use commons_core::spotify::*;
use commons_core::state::SpotifyAuth;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

const ID: &str = "4uLU6hMCjMI75M1A2tKUQC";
type Reply = (u16, String, Vec<(String, String)>);
type Log = Arc<Mutex<Vec<(String, String, String)>>>;

struct Fake { url: String, log: Log }
impl Fake {
    fn calls(&self) -> Vec<String> { self.log.lock().unwrap().iter().map(|(m, p, _)| format!("{m} {p}")).collect() }
    fn bodies(&self) -> Vec<String> { self.log.lock().unwrap().iter().map(|(_, _, b)| b.clone()).collect() }
    fn clear(&self) { self.log.lock().unwrap().clear(); }
}
/// A server on a free loopback port. `h(method, path, body)` makes the answer.
fn fake(h: impl Fn(&str, &str, &str) -> Reply + Send + Sync + 'static) -> Fake {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    let log: Log = Arc::default();
    let (lg, h) = (log.clone(), Arc::new(h));
    std::thread::spawn(move || for mut s in l.incoming().flatten() {
        let mut buf = Vec::new();
        let mut b = [0u8; 4096];
        let (head_end, len) = loop {
            let n = s.read(&mut b).unwrap_or(0);
            if n == 0 { break (0, 0); }
            buf.extend_from_slice(&b[..n]);
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..i]).to_ascii_lowercase();
                let len = head.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|v| v.trim().parse().ok()).unwrap_or(0usize);
                break (i + 4, len);
            }
        };
        if head_end == 0 { continue; }
        while buf.len() < head_end + len { let n = s.read(&mut b).unwrap_or(0); if n == 0 { break; } buf.extend_from_slice(&b[..n]); }
        let text = String::from_utf8_lossy(&buf).into_owned();
        let mut first = text.lines().next().unwrap_or("").split_whitespace();
        let (m, p) = (first.next().unwrap_or("").to_string(), first.next().unwrap_or("").to_string());
        let body = String::from_utf8_lossy(&buf[head_end..(head_end + len).min(buf.len())]).into_owned();
        lg.lock().unwrap().push((m.clone(), p.clone(), body.clone()));
        let (code, rb, hs) = h(&m, &p, &body);
        let extra: String = hs.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
        let _ = write!(s, "HTTP/1.1 {code} X\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{rb}", rb.len());
    });
    Fake { url, log }
}

fn auth() -> SpotifyAuth { SpotifyAuth { access: "AT".into(), refresh: "RT".into(), expires_at: now_ms() / 1000 + 3000 } }
fn target(f: &Fake) -> SpotifyTarget { SpotifyTarget::new(Endpoints { accounts: f.url.clone(), api: f.url.clone() }, "cid".into(), auth()) }
fn sample(pos: u64, now: u64, paused: bool) -> Sample { Sample { play: SpotifyPlay { id: ID.into(), pos_ms: pos, at_ms: now, paused }, rx_ms: now } }
fn player_json(id: &str, playing: bool, pos: u64) -> String { format!(r#"{{"is_playing":{playing},"progress_ms":{pos},"item":{{"id":"{id}"}}}}"#) }
const OK: fn() -> Reply = || (204, String::new(), vec![]);

#[test]
fn the_login_code_is_traded_with_the_pkce_verifier_and_no_secret() {
    let acc = fake(|_, p, _| if p == "/api/token" { (200, r#"{"access_token":"A1","refresh_token":"R1","expires_in":3600}"#.into(), vec![]) } else { (404, String::new(), vec![]) });
    let ep = Endpoints { accounts: acc.url.clone(), api: String::new() };
    let verifier = new_verifier();
    let got = exchange_code(&ep, "cid", "THECODE", "http://127.0.0.1:7420/spotify/callback", &verifier).unwrap();
    assert_eq!((got.access.as_str(), got.refresh.as_str()), ("A1", "R1"));
    let body = acc.bodies().remove(0);
    for part in ["grant_type=authorization_code", "code=THECODE", "client_id=cid", &format!("code_verifier={verifier}"), "redirect_uri=http%3A%2F%2F127.0.0.1%3A7420%2Fspotify%2Fcallback"] { assert!(body.contains(part), "{part} in {body}"); }
    assert!(!body.contains("client_secret"));
    // a refresh without a new refresh token keeps the old one
    let r = refresh(&ep, "cid", "RT-old").unwrap();
    assert_eq!(r.refresh, "R1", "the fake does send one; see next");
    let acc2 = fake(|_, _, _| (200, r#"{"access_token":"A2","expires_in":3600}"#.into(), vec![]));
    let r = refresh(&Endpoints { accounts: acc2.url.clone(), api: String::new() }, "cid", "RT-old").unwrap();
    assert_eq!((r.access.as_str(), r.refresh.as_str()), ("A2", "RT-old"));
    // a refused code is an auth failure, not a crash
    let bad = fake(|_, _, _| (400, r#"{"error":"invalid_grant"}"#.into(), vec![]));
    assert_eq!(exchange_code(&Endpoints { accounts: bad.url.clone(), api: String::new() }, "cid", "x", "r", "v"), Err(Fail::Auth));
}

#[test]
fn one_call_per_change_and_a_drift_check_every_fifteen_seconds() {
    let pos = Arc::new(Mutex::new(0u64));
    let p2 = pos.clone();
    let api = fake(move |m, p, _| match (m, p) {
        ("GET", "/v1/me/player") => (200, player_json(ID, true, *p2.lock().unwrap()), vec![]),
        _ => OK(),
    });
    let mut be = target(&api);
    let mut mem = Mem::default();
    let t0 = 10_000_000u64;
    let s = sample(30_000, t0, false);
    // joining: one play call at the friend's position
    assert_eq!(step(&mut mem, &mut be, &s, t0 + 500), Outcome::default());
    assert_eq!(api.calls(), ["PUT /v1/me/player/play"]);
    let body: serde_json::Value = serde_json::from_str(&api.bodies()[0]).unwrap();
    assert_eq!((body["uris"][0].as_str(), body["position_ms"].as_u64()), (Some(format!("spotify:track:{ID}").as_str()), Some(30_500)));
    api.clear();
    // every second for 14 s: silence (no poll storm)
    for i in 1..15 { step(&mut mem, &mut be, &s, t0 + 500 + i * 1000); }
    assert!(api.calls().is_empty(), "{:?}", api.calls());
    // at 15 s: one read, in sync, so nothing else
    *pos.lock().unwrap() = 30_000 + 15_500;
    step(&mut mem, &mut be, &s, t0 + 15_500);
    assert_eq!(api.calls(), ["GET /v1/me/player"]);
    api.clear();
    // drift of 6 s: one read at the next check, then one seek
    for i in 1..15 { step(&mut mem, &mut be, &s, t0 + 15_500 + i * 1000); }
    *pos.lock().unwrap() = 30_000 + 30_500 - 6_000;
    step(&mut mem, &mut be, &s, t0 + 30_500);
    assert_eq!(api.calls(), ["GET /v1/me/player", "PUT /v1/me/player/seek?position_ms=60500"]);
    api.clear();
    // the friend pauses: one pause call; seeks: one seek call; a new song: one play call
    let paused = sample(61_000, t0 + 31_000, true);
    step(&mut mem, &mut be, &paused, t0 + 31_100);
    assert_eq!(api.calls(), ["PUT /v1/me/player/pause"]);
    api.clear();
    let resumed = sample(90_000, t0 + 40_000, false);
    step(&mut mem, &mut be, &resumed, t0 + 40_100);
    assert_eq!(api.calls(), ["PUT /v1/me/player/play"]);
    api.clear();
    let jumped = sample(200_000, t0 + 50_000, false);
    step(&mut mem, &mut be, &jumped, t0 + 50_100);
    assert_eq!(api.calls(), ["PUT /v1/me/player/seek?position_ms=200100"]);
    api.clear();
    let mut next = sample(0, t0 + 60_000, false); next.play.id = "0VjIjW4GlUZAMYd2vXMi3b".into();
    step(&mut mem, &mut be, &next, t0 + 60_100);
    assert_eq!(api.calls(), ["PUT /v1/me/player/play"]);
    assert!(api.bodies()[0].contains("0VjIjW4GlUZAMYd2vXMi3b"));
}

#[test]
fn starting_something_else_ends_the_following_and_leaves_the_player_alone() {
    let api = fake(|m, p, _| match (m, p) { ("GET", "/v1/me/player") => (200, player_json("0VjIjW4GlUZAMYd2vXMi3b", true, 5), vec![]), _ => OK() });
    let (mut be, mut mem, t0) = (target(&api), Mem::default(), 1_000_000u64);
    let s = sample(0, t0, false);
    step(&mut mem, &mut be, &s, t0);
    api.clear();
    let out = step(&mut mem, &mut be, &s, t0 + CHECK_MS + 1000);
    assert!(out.ended, "{out:?}");
    assert_eq!(api.calls(), ["GET /v1/me/player"], "no pause, no seek: it keeps playing");
}

#[test]
fn no_active_device_starts_on_a_listed_device_or_says_so_without_hammering() {
    let tries = Arc::new(Mutex::new(0));
    let t2 = tries.clone();
    let api = fake(move |m, p, _| match (m, p) {
        ("PUT", "/v1/me/player/play") => { *t2.lock().unwrap() += 1; (404, r#"{"error":{"status":404,"message":"Player command failed: No active device found","reason":"NO_ACTIVE_DEVICE"}}"#.into(), vec![]) }
        ("GET", "/v1/me/player/devices") => (200, r#"{"devices":[{"id":"dev9","is_restricted":false}]}"#.into(), vec![]),
        ("PUT", "/v1/me/player/play?device_id=dev9") => OK(),
        _ => (500, String::new(), vec![]),
    });
    let (mut be, mut mem, t0) = (target(&api), Mem::default(), 1_000_000u64);
    assert_eq!(step(&mut mem, &mut be, &sample(0, t0, false), t0), Outcome::default());
    assert_eq!(api.calls(), ["PUT /v1/me/player/play", "GET /v1/me/player/devices", "PUT /v1/me/player/play?device_id=dev9"]);

    // no device at all
    let none = fake(|m, p, _| match (m, p) {
        ("PUT", _) => (404, r#"{"error":{"reason":"NO_ACTIVE_DEVICE"}}"#.into(), vec![]),
        ("GET", "/v1/me/player/devices") => (200, r#"{"devices":[]}"#.into(), vec![]),
        _ => (500, String::new(), vec![]),
    });
    let (mut be, mut mem) = (target(&none), Mem::default());
    let out = step(&mut mem, &mut be, &sample(0, t0, false), t0);
    assert_eq!((out.ended, out.needs.as_str()), (false, "device"));
    assert!(out.message.contains("Open Spotify"));
    let n = none.calls().len();
    for i in 1..15 { step(&mut mem, &mut be, &sample(0, t0, false), t0 + i * 1000); }
    assert_eq!(none.calls().len(), n, "it waits 15 s before asking again");
    step(&mut mem, &mut be, &sample(0, t0, false), t0 + CHECK_MS + 1);
    assert!(none.calls().len() > n);
}

#[test]
fn a_free_account_falls_back_and_a_rate_limit_is_honoured() {
    let free = fake(|_, _, _| (403, r#"{"error":{"reason":"PREMIUM_REQUIRED"}}"#.into(), vec![]));
    let (mut be, mut mem, t0) = (target(&free), Mem::default(), 1_000_000u64);
    let out = step(&mut mem, &mut be, &sample(0, t0, false), t0);
    assert!(out.ended && out.needs == "premium" && out.message.contains("Premium"), "{out:?}");

    let count = Arc::new(Mutex::new(0));
    let c2 = count.clone();
    let lim = fake(move |_, _, _| { let mut c = c2.lock().unwrap(); *c += 1; if *c == 1 { (429, String::new(), vec![("Retry-After".into(), "7".into())]) } else { (204, String::new(), vec![]) } });
    let (mut be, mut mem) = (target(&lim), Mem::default());
    let out = step(&mut mem, &mut be, &sample(0, t0, false), t0);
    assert!(!out.ended && out.message.contains("7 seconds"), "{out:?}");
    for i in 1..7 { step(&mut mem, &mut be, &sample(0, t0, false), t0 + i * 1000); }
    assert_eq!(*count.lock().unwrap(), 1, "nothing is sent while Spotify asked for quiet");
    step(&mut mem, &mut be, &sample(0, t0, false), t0 + 7_600);
    assert_eq!(*count.lock().unwrap(), 2, "then it tries once");
}

#[test]
fn an_expired_login_is_refreshed_and_a_dead_one_is_reported() {
    let seen = Arc::new(Mutex::new(vec![]));
    let s2 = seen.clone();
    let srv = fake(move |m, p, body| {
        s2.lock().unwrap().push(format!("{m} {p}"));
        match p {
            "/api/token" if body.contains("RT") => (200, r#"{"access_token":"FRESH","expires_in":3600}"#.into(), vec![]),
            "/v1/me/player/play" => OK(),
            _ => (500, String::new(), vec![]),
        }
    });
    let mut stale = auth(); stale.expires_at = 1;
    let mut be = SpotifyTarget::new(Endpoints { accounts: srv.url.clone(), api: srv.url.clone() }, "cid".into(), stale);
    let (mut mem, t0) = (Mem::default(), 1_000_000u64);
    step(&mut mem, &mut be, &sample(0, t0, false), t0);
    assert_eq!(*seen.lock().unwrap(), ["POST /api/token", "PUT /v1/me/player/play"]);
    let kept = be.auth().expect("the new token is handed back to be saved");
    assert_eq!((kept.access.as_str(), kept.refresh.as_str()), ("FRESH", "RT"));

    let dead = fake(|_, p, _| if p == "/api/token" { (400, r#"{"error":"invalid_grant"}"#.into(), vec![]) } else { (401, String::new(), vec![]) });
    let mut a = auth(); a.expires_at = 1;
    let mut be = SpotifyTarget::new(Endpoints { accounts: dead.url.clone(), api: dead.url.clone() }, "cid".into(), a);
    let out = step(&mut Mem::default(), &mut be, &sample(0, t0, false), t0);
    assert!(out.ended && out.needs == "login", "{out:?}");
}

#[test]
fn rune_is_driven_through_its_own_player_api() {
    let state = Arc::new(Mutex::new(format!(r#"{{"status":"playing","index":0,"positionMs":40000,"updatedAt":{},"queue":[{{"track":{{"id":"sp:{ID}","source":"spotify","spotifyUri":"spotify:track:{ID}","title":"x","artists":[]}}}}]}}"#, now_ms())));
    let st = state.clone();
    let rune = fake(move |m, p, _| match (m, p) {
        ("GET", "/api/zones") => (200, r#"[{"id":"pc","name":"PC speakers","kind":"pc"},{"id":"phone","name":"Phone","kind":"web"}]"#.into(), vec![]),
        ("GET", "/api/player/pc") => (200, st.lock().unwrap().clone(), vec![]),
        ("POST", "/api/player/pc") => (200, "{}".into(), vec![]),
        _ => (404, String::new(), vec![]),
    });
    let zones = commons_core::rune::zones(&rune.url).unwrap();
    assert_eq!(zones[0], ("pc".to_string(), "PC speakers".to_string()));
    rune.clear();
    let mut be = RuneTarget { base: rune.url.clone(), zone: "pc".into(), name: "PC speakers".into() };
    assert_eq!(be.label(), "rune (PC speakers)");
    let (mut mem, t0) = (Mem::default(), now_ms());
    step(&mut mem, &mut be, &sample(30_000, t0, false), t0);
    let sent: serde_json::Value = serde_json::from_str(&rune.bodies()[0]).unwrap();
    assert_eq!((sent["type"].as_str(), sent["trackIds"][0].as_str(), sent["positionMs"].as_u64(), sent["paused"].as_bool()), (Some("play"), Some(format!("sp:{ID}").as_str()), Some(30_000), Some(false)));
    rune.clear();
    for i in 1..15 { step(&mut mem, &mut be, &sample(30_000, t0, false), t0 + i * 1000); }
    assert!(rune.calls().is_empty(), "no poll storm: {:?}", rune.calls());
    // the check reads rune (40 s in; the friend is at 45 s: within the limit? 5 s off, so it seeks)
    step(&mut mem, &mut be, &sample(30_000, t0, false), t0 + 15_000);
    assert_eq!(rune.calls(), ["GET /api/player/pc", "POST /api/player/pc"]);
    assert_eq!(serde_json::from_str::<serde_json::Value>(&rune.bodies()[1]).unwrap()["type"], "seek");
    rune.clear();
    step(&mut mem, &mut be, &sample(61_000, t0 + 20_000, true), t0 + 20_100);
    assert_eq!(serde_json::from_str::<serde_json::Value>(&rune.bodies()[0]).unwrap()["type"], "pause");
    // a rune that is not there is an ordinary failure, reported, not a panic
    let mut gone = RuneTarget { base: "http://127.0.0.1:1".into(), zone: "pc".into(), name: "x".into() };
    let out = step(&mut Mem::default(), &mut gone, &sample(0, t0, false), t0);
    assert!(!out.ended && out.message.contains("Could not reach"));
}
