//! Ringing a voice room: one more encrypted envelope kind, `Ring`, which an older app skips as "newer version". What the
//! receiver enforces (member, fresh, rate, not yourself) is `ring_check`, tested here alone and then end to end.
mod common;
use common::*;
use commons_core::browser_update::now_secs;
use commons_core::envelope::*;

const ME: &str = "aa";
fn id(n: u8) -> String { format!("{:032x}", n) }
fn key(n: u8) -> String { format!("{:064x}", n) }

#[test]
fn ring_validation_rules() {
    let now = 1_000_000;
    let ok = |ts, sender, to, member, recent: &[u64]| ring_check(now, ts, sender, ME, to, member, recent);
    assert_eq!(ok(now, "bb", None, true, &[]), RingVerdict::Show);
    assert_eq!(ok(now - RING_MAX_AGE, "bb", None, true, &[]), RingVerdict::Show, "exactly at the limit still rings");
    assert_eq!(ok(now - RING_MAX_AGE - 1, "bb", None, true, &[]), RingVerdict::Stale, "a device catching up never rings for an old call");
    assert_eq!(ok(now + RING_MAX_AHEAD + 1, "bb", None, true, &[]), RingVerdict::Stale, "nor one from far in the future");
    assert_eq!(ok(now, ME, None, true, &[]), RingVerdict::Mine, "never ring yourself or your other devices");
    assert_eq!(ok(now, "bb", None, false, &[]), RingVerdict::NotMember);
    assert_eq!(ok(now, "bb", Some("cc"), true, &[]), RingVerdict::NotForMe);
    assert_eq!(ok(now, "bb", Some(ME), true, &[]), RingVerdict::Show);
    let many: Vec<u64> = (0..RING_PER_MIN as u64).map(|i| now - i).collect();
    assert_eq!(ok(now, "bb", None, true, &many), RingVerdict::TooMany);
    let old: Vec<u64> = (0..RING_PER_MIN as u64).map(|i| now - 61 - i).collect();
    assert_eq!(ok(now, "bb", None, true, &old), RingVerdict::Show, "a minute later the limit is open again");
}

#[test]
fn ring_envelope_parses_and_old_apps_skip_it() {
    let e = Envelope::Ring { id: id(1), ts: 5, to: Some(key(2)), end: false };
    assert_eq!(Envelope::parse(&e.to_bytes()).unwrap(), e);
    let plain = Envelope::Ring { id: id(1), ts: 5, to: None, end: true };
    assert_eq!(Envelope::parse(&plain.to_bytes()).unwrap(), plain);
    assert!(Envelope::parse(&Envelope::Ring { id: "nope".into(), ts: 5, to: None, end: false }.to_bytes()).is_err());
    assert!(Envelope::parse(&Envelope::Ring { id: id(1), ts: 5, to: Some("x".into()), end: false }.to_bytes()).is_err());
    // What an app from before rings does with it: it is `Unknown` there ("newer version"), and no field is needed to say so.
    #[derive(serde::Deserialize, Debug, PartialEq)]
    #[serde(tag = "t")]
    enum Old { Typing, #[serde(other)] Unknown }
    assert_eq!(serde_json::from_slice::<Old>(&e.to_bytes()).unwrap(), Old::Unknown);
}

#[test]
fn a_ring_reaches_the_room_but_not_the_caller_and_stale_ones_ring_nobody() {
    let net = Net::new();
    let (mut alice, mut bob, mut carol) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"), Peer::new(&net, "carol"));
    let (cid, gen) = community_with(&net, &mut alice, &mut [&mut bob, &mut carol]);
    let lounge = alice.c.create_channel_kind(&cid, "lounge", ROOM_VOICE).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    // only a voice room rings, and not yourself
    assert!(alice.c.ring_send(&gen, None).is_err(), "a text room has no ring");
    assert!(alice.c.ring_send(&lounge, Some(&alice.acct())).is_err());
    let r = alice.c.ring_send(&lounge, None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    for p in [&bob, &carol] {
        let ev = p.c.ring_poll(0);
        assert_eq!(ev.len(), 1, "{}", p.name);
        assert_eq!((ev[0].gid.as_str(), ev[0].account.as_str(), ev[0].ring.as_str(), ev[0].end, ev[0].direct), (lounge.as_str(), alice.acct().as_str(), r.as_str(), false, false));
    }
    assert!(alice.c.ring_poll(0).is_empty(), "the caller is never rung");
    // one person only: nobody else hears it
    let to_bob = alice.c.ring_send(&lounge, Some(&bob.acct())).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert_eq!(bob.c.ring_poll(0).len(), 2);
    assert!(bob.c.ring_poll(0)[1].direct);
    assert_eq!(carol.c.ring_poll(0).len(), 1, "carol was not the one asked");
    // taking it back
    alice.c.ring_end(&lounge, &to_bob, Some(&bob.acct())).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    let last = bob.c.ring_poll(0).pop().unwrap();
    assert!(last.end && last.ring == to_bob);
    // a ring the caller cannot repeat at once
    assert!(alice.c.ring_send(&lounge, None).is_err(), "twice in five seconds is refused on the sending side");
    // a stale ring (a device catching up on an old log) and one from the future ring nobody
    let seen = bob.c.ring_poll(0).last().unwrap().id;
    alice.c.debug_send_envelope(&lounge, Envelope::Ring { id: id(7), ts: now_secs() - 120, to: None, end: false }).unwrap();
    alice.c.debug_send_envelope(&lounge, Envelope::Ring { id: id(8), ts: now_secs() + 3600, to: None, end: false }).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert!(bob.c.ring_poll(seen).is_empty(), "stale and future rings are ignored: {:?}", bob.c.ring_poll(seen));
    // flooding: a member sending many is capped per minute, and the same id twice rings once
    let before = carol.c.ring_poll(0).len();
    for n in 20..40 { alice.c.debug_send_envelope(&lounge, Envelope::Ring { id: id(n), ts: now_secs(), to: None, end: false }).unwrap(); settle(&mut [&mut alice, &mut bob, &mut carol]); }
    let got = carol.c.ring_poll(0).into_iter().filter(|e| !e.end).count();
    assert!(got <= RING_PER_MIN, "{got} rings in a minute from one sender (had {before})");
    // a text room carries no ring even if one is sent
    alice.c.debug_send_envelope(&gen, Envelope::Ring { id: id(99), ts: now_secs(), to: None, end: false }).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert!(bob.c.ring_poll(0).iter().all(|e| e.gid == lounge), "a ring in a text room is ignored");
}
