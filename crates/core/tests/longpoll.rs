//! The sync poller (api.rs runs one on its own thread): a request the relay holds until there is news, and the ordinary
//! round that syncs only for what that request does not cover.
mod common;
use common::*;
use std::time::{Duration, Instant};

fn syncs(p: &Peer) -> u32 { p.faults.lock().unwrap().syncs }
fn live_polls(p: &Peer) -> u32 { p.faults.lock().unwrap().live_polls }

#[test]
fn a_held_sync_returns_as_soon_as_a_message_is_posted() {
    let net = Net::new();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (_cid, gen) = community_with(&net, &mut a, &mut [&mut b]);
    settle(&mut [&mut a, &mut b]);
    // Bob's poller asks with a long wait and nothing is new: the relay holds the request.
    let plan = b.c.sync_plan(8_000).expect("bob has an account to poll for");
    let t = Instant::now();
    let held = std::thread::spawn(move || { let d = plan.run(); (d, t.elapsed()) });
    std::thread::sleep(Duration::from_millis(700));
    assert!(!held.is_finished(), "nothing was posted, so the relay still holds the poll");
    a.c.send_text(&gen, "are you there", None).unwrap();
    a.tick();
    let (d, took) = held.join().unwrap();
    assert!(took < Duration::from_secs(4), "the poll came back as soon as the message was posted, not at the deadline ({took:?})");
    assert!(d.got_any() && !d.failed());
    b.c.finish_sync(d);
    assert!(has_text(&b, &gen, "are you there"));
}

#[test]
fn the_round_syncs_only_for_groups_the_held_poll_does_not_cover() {
    let net = Net::new();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (_cid, _gen) = community_with(&net, &mut a, &mut [&mut b]);
    settle(&mut [&mut a, &mut b]);
    // The poller has a request out for every group bob is in (planned, not run: the groups are what matters here).
    let _plan = b.c.sync_plan(25_000).unwrap();
    let before = syncs(&b);
    b.tick();
    assert_eq!(syncs(&b), before, "the round did not sync: the held poll covers every group");
    // A group the held poll does not know: the round syncs for it at once rather than waiting for the poll to return.
    b.c.create_community("bob's own").unwrap();
    b.tick();
    assert!(syncs(&b) > before, "the round synced for the new group");
    // The poller's next request covers it, and the round is quiet again.
    let _plan = b.c.sync_plan(25_000).unwrap();
    let before = syncs(&b);
    b.tick();
    assert_eq!(syncs(&b), before);
    // Without a poller (tests, the synchronous tick) every round syncs, as it always did.
    let mut c = Peer::new(&net, "carol");
    let before = syncs(&c);
    c.tick();
    assert!(syncs(&c) > before);
}

#[test]
fn a_room_joined_while_the_live_poll_is_held_is_polled_by_the_round() {
    let net = Net::new();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (cid, gen) = community_with(&net, &mut a, &mut [&mut b]);
    settle(&mut [&mut a, &mut b]);
    a.c.live_send(&gen, "warm", "1").unwrap();
    settle(&mut [&mut a, &mut b]);
    // The app's poller thread is waiting on a poll for the one room: the round leaves polling to it.
    b.c.set_live_background(true);
    b.c.set_live_polling(vec![gen.clone()]);
    let before = live_polls(&b);
    b.tick();
    assert_eq!(live_polls(&b), before, "the round does not poll what the held poll covers");
    // A second room appears: the round polls (without waiting) until the poller's next request covers it too.
    let gid2 = a.c.create_channel(&cid, "second").unwrap();
    settle(&mut [&mut a, &mut b]);
    assert!(b.c.st.groups.contains_key(&gid2), "bob is in the new room");
    let before = live_polls(&b);
    b.tick();
    assert!(live_polls(&b) > before, "the round polled for the new room");
    b.c.set_live_polling(vec![gen.clone(), gid2.clone()]);
    let before = live_polls(&b);
    b.tick();
    assert_eq!(live_polls(&b), before);
}
