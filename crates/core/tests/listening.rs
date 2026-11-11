//! "Listening to" status: shared through a profile refresh, so an older Megatron reads it as a profile and never
//! sees a message kind it does not know.
mod common;
use common::*;
use commons_core::envelope::*;
use commons_proto::now_secs;

/// The envelope an older Megatron (0.2.x) parses: the same shape, without the extension field. If a status arrived
/// as some new kind, it would land in `Unknown` and the room would say so every time the song changed.
#[derive(serde::Deserialize, Debug)]
#[serde(tag = "t")]
enum OldEnvelope {
    Profile { name: String, ts: u64 },
    #[serde(other)]
    Unknown,
}

fn pair(net: &Net) -> (Peer, Peer, String, String) {
    let (mut a, mut b) = (Peer::new(net, "alice"), Peer::new(net, "bob"));
    let (cid, gen) = community_with(net, &mut a, &mut [&mut b]);
    (a, b, cid, gen)
}

fn sea_power() -> NowPlaying { NowPlaying::text("Detective Arriving on the Scene", "Sea Power") }

#[test]
fn a_shared_status_reaches_a_friend_encrypted() {
    let net = Net::new();
    let (mut a, mut b, cid, _gen) = pair(&net);
    a.c.set_listening_sharing(true, "http://127.0.0.1:1").unwrap();
    a.c.update_listening(Some(sea_power())).unwrap();
    settle(&mut [&mut a, &mut b]);

    assert_eq!(b.c.listening_of(&a.acct()), Some(&sea_power()), "bob sees what alice plays");
    let row = b.c.members(&cid).into_iter().find(|m| m.account == a.acct()).unwrap();
    assert_eq!(row.listening, Some(sea_power()), "and it is on her row in the people list");

    // Nothing about it is readable on the relay: it travelled inside the group's encryption like everything else.
    let disk = relay_bytes(&net);
    assert!(!contains(&disk, "Detective Arriving on the Scene"));
    assert!(!contains(&disk, "Sea Power"));
    // And it did not look like a newer app's message kind to anyone.
    assert!(!notices(&b).iter().any(|n| n.contains("newer") || n.contains("does not know")), "{:?}", notices(&b));
}

#[test]
fn a_status_clears_when_playback_stops_and_expires_on_its_own() {
    let net = Net::new();
    let (mut a, mut b, _cid, _gen) = pair(&net);
    a.c.set_listening_sharing(true, "http://127.0.0.1:1").unwrap();
    a.c.update_listening(Some(sea_power())).unwrap();
    settle(&mut [&mut a, &mut b]);
    assert!(b.c.listening_of(&a.acct()).is_some());

    // An app that vanishes (closed, crashed, laptop shut) never sends a stop: the TTL is the stop.
    b.c.st.listening.get_mut(&a.acct()).unwrap().ts = now_secs().saturating_sub(LISTENING_TTL_SECS + 1);
    assert_eq!(b.c.listening_of(&a.acct()), None, "a stale status is not shown");

    // Playing again, then paused/stopped: that is a change, and it is sent at once.
    a.c.update_listening(None).unwrap();
    a.c.update_listening(Some(sea_power())).unwrap();
    settle(&mut [&mut a, &mut b]);
    assert!(b.c.listening_of(&a.acct()).is_some());
    a.c.update_listening(None).unwrap();
    settle(&mut [&mut a, &mut b]);
    assert_eq!(b.c.listening_of(&a.acct()), None, "stopping clears it everywhere");
}

#[test]
fn an_older_megatron_sees_a_profile_and_never_a_kind_it_does_not_know() {
    let net = Net::new();
    let (mut a, mut b, _cid, _gen) = pair(&net);
    a.c.set_listening_sharing(true, "http://127.0.0.1:1").unwrap();
    a.c.update_listening(Some(sea_power())).unwrap();

    // The exact plaintext that is about to go into the encrypted log.
    let sent = a.c.st.outbox.last().expect("a profile is queued").env.to_bytes();
    let old: OldEnvelope = serde_json::from_slice(&sent).expect("an older version can parse it");
    match old {
        OldEnvelope::Profile { name, ts } => { assert_eq!(name, "alice"); assert!(ts > 0); }
        OldEnvelope::Unknown => panic!("an older version would call this an unknown kind and show a notice"),
    }
    // The newer version reads the same bytes, with the status.
    assert!(matches!(Envelope::parse(&sent).unwrap(), Envelope::Profile { listen: Some(ref l), .. } if l == &sea_power()));

    // The other direction: a profile written by an older version has no such field, and is still understood.
    let older = br#"{"t":"Profile","name":"alice","ts":5}"#;
    assert!(matches!(Envelope::parse(older).unwrap(), Envelope::Profile { listen: None, .. }));
    // And when bob, who runs this version, receives alice's status, he does not report anything odd.
    settle(&mut [&mut a, &mut b]);
    assert!(!notices(&b).iter().any(|n| n.contains("newer")), "{:?}", notices(&b));
}

#[test]
fn sending_is_on_change_only_and_renews_slowly() {
    let net = Net::new();
    let (mut a, _b, _cid, _gen) = pair(&net);
    a.c.set_listening_sharing(true, "http://127.0.0.1:1").unwrap();
    let base = a.c.st.outbox.len();

    let song = sea_power();
    a.c.update_listening(Some(song.clone())).unwrap();
    assert_eq!(a.c.st.outbox.len(), base + 1, "a new status goes out once per community");
    a.c.update_listening(Some(song.clone())).unwrap();
    a.c.update_listening(Some(song.clone())).unwrap();
    assert_eq!(a.c.st.outbox.len(), base + 1, "polling the same song again sends nothing");

    let next = NowPlaying::text("Waving Flags", "Sea Power");
    a.c.update_listening(Some(next.clone())).unwrap();
    assert_eq!(a.c.st.outbox.len(), base + 2, "a new song is a change");

    // Five quiet minutes still playing: renew, so the receivers' TTL does not cut a live status short.
    a.c.st.listening_sent_ts = now_secs().saturating_sub(LISTENING_REFRESH_SECS);
    a.c.update_listening(Some(next)).unwrap();
    assert_eq!(a.c.st.outbox.len(), base + 3, "a still-playing status is renewed");

    a.c.update_listening(None).unwrap();
    assert_eq!(a.c.st.outbox.len(), base + 4, "stopping is a change");
    a.c.update_listening(None).unwrap();
    assert_eq!(a.c.st.outbox.len(), base + 4, "staying stopped sends nothing");

    // Off by default: nothing is looked at, and nothing is sent.
    let mut c = Peer::new(&net, "carol");
    let quiet = c.c.st.outbox.len();
    c.c.update_listening(Some(sea_power())).unwrap();
    assert_eq!(c.c.st.outbox.len(), quiet);
    assert_eq!(c.c.listening_of(&c.acct()), None);
}

#[test]
fn turning_sharing_off_tells_the_room_and_defaults_are_sane() {
    let net = Net::new();
    let (mut a, mut b, _cid, _gen) = pair(&net);
    a.c.set_listening_sharing(true, "").unwrap();
    assert_eq!(a.c.rune_url(), commons_core::rune::DEFAULT_URL);
    assert_eq!(a.c.rune_url(), "http://127.0.0.1:4545");
    a.c.update_listening(Some(sea_power())).unwrap();
    settle(&mut [&mut a, &mut b]);
    assert!(b.c.listening_of(&a.acct()).is_some());
    a.c.set_listening_sharing(false, "").unwrap();
    settle(&mut [&mut a, &mut b]);
    assert_eq!(b.c.listening_of(&a.acct()), None, "switching off clears it now, not in ten minutes");
    assert_eq!(a.c.listening_of(&a.acct()), None);
}

// ---- rune over its own HTTP API, against a fake server (never the real player)
fn fake_rune(routes: Vec<(&'static str, &'static str)>) -> String {
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let mut buf = [0u8; 8192];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let path = req.split_whitespace().nth(1).unwrap_or("");
            let found = routes.iter().find(|(p, _)| *p == path);
            let (code, body) = match found { Some((_, b)) => ("200 OK", *b), None => ("404 Not Found", "{\"error\":\"no\"}") };
            let _ = write!(s, "HTTP/1.1 {code}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
        }
    });
    format!("http://{addr}")
}

#[test]
fn rune_reports_the_zone_that_is_actually_playing() {
    let playing = fake_rune(vec![
        ("/api/zones", r#"[{"id":"pc","name":"Linux PC speakers","kind":"pc"},{"id":"spotify:phone","name":"Phone","kind":"spotify"}]"#),
        ("/api/player/pc", r#"{"status":"paused","index":0,"queue":[{"track":{"title":"Old","artists":[{"name":"Nobody"}]}}]}"#),
        ("/api/player/spotify:phone", r#"{"status":"playing","index":0,"queue":[{"track":{"title":"Detective Arriving on the Scene","artists":[{"name":"Sea Power"}]}}]}"#),
    ]);
    assert_eq!(commons_core::rune::now_playing(&playing).unwrap(), Some(sea_power()));

    // The PC speakers win when they are the ones playing.
    let local = fake_rune(vec![
        ("/api/zones", r#"[{"id":"spotify:phone","name":"Phone","kind":"spotify"},{"id":"pc","name":"Linux PC speakers","kind":"pc"}]"#),
        ("/api/player/spotify:phone", r#"{"status":"playing","index":0,"queue":[{"track":{"title":"Phone song","artists":[{"name":"X"}]}}]}"#),
        ("/api/player/pc", r#"{"status":"playing","index":0,"queue":[{"track":{"title":"Waving Flags","artists":[{"name":"Sea Power"}]}}]}"#),
    ]);
    assert_eq!(commons_core::rune::now_playing(&local).unwrap().unwrap().title, "Waving Flags");

    // Idle is Ok(None); unreachable is an error and must not be mistaken for "stopped".
    let idle = fake_rune(vec![("/api/zones", "[]")]);
    assert_eq!(commons_core::rune::now_playing(&idle).unwrap(), None);
    assert!(commons_core::rune::now_playing("http://127.0.0.1:1").is_err());
}

#[test]
fn a_spotify_status_carries_the_position_and_keeps_its_arrival_time() {
    let net = Net::new();
    let (mut a, mut b, _cid, _gen) = pair(&net);
    a.c.set_listening_sharing(true, "http://127.0.0.1:1").unwrap();
    let t0 = commons_core::spotify::now_ms();
    let mk = |pos: u64, at: u64, paused: bool| NowPlaying { title: "Song".into(), artist: "Band".into(), spotify: Some(SpotifyPlay { id: "4uLU6hMCjMI75M1A2tKUQC".into(), pos_ms: pos, at_ms: at, paused }) };
    a.c.update_listening(Some(mk(10_000, t0, false))).unwrap();
    settle(&mut [&mut a, &mut b]);
    let got = b.c.listening_of(&a.acct()).unwrap().clone();
    assert_eq!(got.spotify.as_ref().map(|s| (s.pos_ms, s.paused)), Some((10_000, false)));
    let (_, rx) = b.c.listening_sample(&a.acct()).unwrap();
    assert!(rx >= t0, "the arrival time is ours");

    // plain progress is not sent (nothing new reaches bob); a pause and a seek are
    let sent_before = a.c.st.listening_sent_ts;
    a.c.update_listening(Some(mk(14_000, t0 + 4_000, false))).unwrap();
    assert_eq!(a.c.st.listening_sent.as_ref().unwrap().spotify.as_ref().unwrap().pos_ms, 10_000, "progress alone sends nothing");
    a.c.update_listening(Some(mk(14_500, t0 + 4_500, true))).unwrap();
    settle(&mut [&mut a, &mut b]);
    assert!(b.c.listening_of(&a.acct()).unwrap().spotify.as_ref().unwrap().paused, "a pause is sent");
    a.c.update_listening(Some(mk(300_000, t0 + 9_000, true))).unwrap();
    settle(&mut [&mut a, &mut b]);
    assert_eq!(b.c.listening_of(&a.acct()).unwrap().spotify.as_ref().unwrap().pos_ms, 300_000, "a seek while paused is sent");
    let _ = sent_before;

    // switching to a local file drops the Spotify part
    a.c.update_listening(Some(NowPlaying::text("Local song", "Someone"))).unwrap();
    settle(&mut [&mut a, &mut b]);
    assert_eq!(b.c.listening_of(&a.acct()).unwrap().spotify, None);

    // a paused Spotify song is only reported as the end of a playing one, never as a fresh status
    a.c.update_listening(None).unwrap();
    a.c.update_listening(Some(mk(5_000, t0 + 20_000, true))).unwrap();
    settle(&mut [&mut a, &mut b]);
    assert_eq!(b.c.listening_of(&a.acct()), None, "a song that was never playing here is not announced as paused");
}
