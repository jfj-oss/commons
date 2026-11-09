//! The unstored live channel (crates/core/src/live.rs, crates/relay/src/live.rs): two clients exchange live events with
//! nothing added to the relay's log and no MLS state touched; a member who missed thousands of them still reads the next
//! real message; a relay or a client from before the channel still works next to one that has it.
mod common;
use commons_core::live::Support;
use commons_proto::*;
use common::*;

fn live(p: &Peer, since: u64) -> Vec<(String, String, String)> { p.c.live_poll(since, None).into_iter().map(|e| (e.account, e.kind, e.data)).collect() }
fn entries(net: &Net, gid: &str) -> usize { relay_entries(net, gid).len() }
fn state_file(p: &Peer) -> Vec<u8> { std::fs::read(p.dir.path().join("state.cmns")).unwrap() }
fn trio(net: &Net) -> (Peer, Peer, Peer, String, String) {
    let (mut a, mut b, mut c) = (Peer::new(net, "alice"), Peer::new(net, "bob"), Peer::new(net, "carol"));
    let (cid, gen) = community_with(net, &mut a, &mut [&mut b, &mut c]);
    settle(&mut [&mut a, &mut b, &mut c]);
    (a, b, c, cid, gen)
}
fn old_relay() -> Net { Net::with_limits(commons_relay::Limits { live: commons_relay::live::LiveLimits { enabled: false, ..Default::default() }, ..Default::default() }) }

#[test]
fn two_clients_exchange_events_and_nothing_is_logged_or_changed() {
    let net = Net::new();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (_cid, gen) = community_with(&net, &mut a, &mut [&mut b]);
    settle(&mut [&mut a, &mut b]);
    // One exchange first, so that both know the relay carries the channel and every room has its cursor.
    a.c.live_send(&gen, "warm", "1").unwrap();
    settle(&mut [&mut a, &mut b]);
    assert_eq!(a.c.live_support(), Support::Yes);
    assert_eq!(live(&b, 0).len(), 1);

    let log_before = entries(&net, &gen);
    let (mls_a, mls_b) = (a.c.debug_mls_dump(), b.c.debug_mls_dump());
    let (file_a, file_b) = (state_file(&a), state_file(&b));
    let epoch_a = a.c.st.groups[&gen].cursor;
    for i in 0..40 {
        a.c.live_send(&gen, "tile", &format!("{{\"x\":{i}}}")).unwrap();
        if i % 2 == 0 { b.c.live_send(&gen, "back", &format!("{i}")).unwrap(); }
        a.tick(); b.tick();
        std::thread::sleep(std::time::Duration::from_millis(60));
    }
    settle(&mut [&mut a, &mut b]);
    let got_b: Vec<_> = live(&b, 0).into_iter().filter(|e| e.1 == "tile").collect();
    let got_a: Vec<_> = live(&a, 0).into_iter().filter(|e| e.1 == "back").collect();
    assert!(got_b.len() >= 10 && got_a.len() >= 8, "events arrived ({} and {})", got_b.len(), got_a.len());
    assert_eq!(got_b[0].0, a.acct(), "from alice, as the group vouches");
    assert!(a.c.live_stats().sealed >= 15 && b.c.live_stats().taken >= 10, "{:?} {:?}", a.c.live_stats(), b.c.live_stats());
    // Not one entry in the relay's log, and no MLS state or saved state moved on either side.
    assert_eq!(entries(&net, &gen), log_before, "nothing added to the log");
    assert_eq!(a.c.debug_mls_dump(), mls_a, "alice's MLS state is untouched");
    assert_eq!(b.c.debug_mls_dump(), mls_b, "bob's MLS state is untouched");
    assert_eq!(state_file(&a), file_a, "alice's saved state was not rewritten");
    assert_eq!(state_file(&b), file_b, "bob's saved state was not rewritten");
    assert_eq!(a.c.st.groups[&gen].cursor, epoch_a);
    assert_eq!(a.c.outbox_len(), 0);
    assert!(!notices(&b).iter().any(|n| n.contains("newer")), "{:?}", notices(&b));
    // Nothing readable and nothing in the database for these events.
    assert!(!contains(&relay_bytes(&net), "tile"));
}

#[test]
fn a_member_who_missed_three_thousand_live_events_still_reads_the_next_message() {
    // The relay is told to hold a lot, so all of them really are accepted (the point is that none touches a ratchet).
    let lim = commons_relay::live::LiveLimits { per_sec: 1_000_000, group_events: 10_000, sender_events: 10_000, sender_bytes: 64 << 20, group_bytes: 64 << 20, ..Default::default() };
    let net = Net::with_limits(commons_relay::Limits { live: lim, ..Default::default() });
    let (mut a, mut b, mut c, _cid, gen) = trio(&net);
    a.c.live_send(&gen, "warm", "1").unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    // Carol goes away (her phone is off); alice sends 3000 live events, more than the 2000 steps a ratchet allows to be skipped.
    c.faults.lock().unwrap().offline = true;
    let sent = a.c.debug_live_burst(&gen, 3000);
    assert_eq!(sent, 3000);
    assert!(a.c.live_stats().posts >= 150, "posted in batches: {}", a.c.live_stats().posts);
    assert_eq!(entries(&net, &gen), entries(&net, &gen));
    a.c.send_text(&gen, "are you there?", None).unwrap();
    settle(&mut [&mut a, &mut b]);
    // Carol comes back and reads the real message; her group is not broken.
    c.faults.lock().unwrap().offline = false;
    settle(&mut [&mut a, &mut b, &mut c]);
    assert!(has_text(&c, &gen, "are you there?"), "carol reads the next real message");
    let g = &c.c.st.groups[&gen];
    assert!(!g.broken && !g.tainted, "her view of the group is intact");
    // And she can answer, and alice can read it.
    c.c.send_text(&gen, "yes", None).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    assert!(has_text(&a, &gen, "yes") && has_text(&b, &gen, "yes"));
    assert!(!notices(&c).iter().any(|n| n.contains("decrypt")), "{:?}", notices(&c));
}

#[test]
fn a_relay_without_the_channel_is_detected_and_the_log_carries_live_events_at_the_old_limit() {
    let net = old_relay();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (_cid, gen) = community_with(&net, &mut a, &mut [&mut b]);
    settle(&mut [&mut a, &mut b]);
    assert!(matches!(a.c.live_support(), Support::No(_)) && matches!(b.c.live_support(), Support::No(_)), "{:?}", a.c.live_support());
    let before = entries(&net, &gen);
    for i in 0..20 { a.c.live_send(&gen, "cursor", &format!("{i}")).unwrap(); }
    assert!(a.c.outbox_len() <= 4, "four a second through the log, got {}", a.c.outbox_len());
    settle(&mut [&mut a, &mut b]);
    let got = live(&b, 0);
    assert!(!got.is_empty() && got.len() <= 4, "{got:?}");
    assert_eq!(got[0].2, "0");
    assert!(entries(&net, &gen) > before, "through the log, as before");
    // Chat is untouched.
    a.c.send_text(&gen, "hello", None).unwrap();
    settle(&mut [&mut a, &mut b]);
    assert!(has_text(&b, &gen, "hello"));
    // And no error shows anywhere because of the missing channel.
    assert!(!notices(&a).iter().any(|n| n.contains("404") || n.contains("live")), "{:?}", notices(&a));
}

#[test]
fn the_same_through_the_fault_that_makes_any_relay_answer_404() {
    let net = Net::new();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (_cid, gen) = community_with(&net, &mut a, &mut [&mut b]);
    settle(&mut [&mut a, &mut b]);
    for p in [&a, &b] { p.faults.lock().unwrap().no_live = true; }
    settle(&mut [&mut a, &mut b]);
    assert!(matches!(a.c.live_support(), Support::No(_)));
    a.c.live_send(&gen, "cursor", "7").unwrap();
    settle(&mut [&mut a, &mut b]);
    assert_eq!(live(&b, 0).len(), 1);
    // The relay is upgraded: after the recheck time both go back to the channel.
    for p in [&a, &b] { p.faults.lock().unwrap().no_live = false; }
    a.c.debug_live_recheck_now(); b.c.debug_live_recheck_now();
    settle(&mut [&mut a, &mut b]);
    assert_eq!(a.c.live_support(), Support::Yes);
    let log = entries(&net, &gen);
    a.c.live_send(&gen, "cursor", "8").unwrap();
    settle(&mut [&mut a, &mut b]);
    assert_eq!(entries(&net, &gen), log, "no longer in the log");
    assert!(live(&b, 0).iter().any(|e| e.2 == "8"));
}

#[test]
fn a_client_from_before_the_channel_keeps_working_next_to_a_new_one_on_a_new_relay() {
    let net = Net::new();
    let (mut a, mut b, mut c, _cid, gen) = trio(&net);
    b.c.debug_live_legacy(true); // bob is a 0.3.0
    a.c.live_send(&gen, "warm", "1").unwrap();
    c.c.live_send(&gen, "warm", "1").unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    // New to new works; the old client sees nothing from them, and nothing breaks or is reported.
    assert!(live(&c, 0).iter().any(|e| e.0 == a.acct()));
    assert!(live(&b, 0).is_empty(), "an old client does not see events of the new channel");
    assert!(notices(&b).iter().all(|n| !n.contains("live") && !n.contains("404")), "{:?}", notices(&b));
    // Old to new: bob's events go through the log, as they always did, and the new clients still read them.
    let log = entries(&net, &gen);
    b.c.live_send(&gen, "cursor", "from bob").unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    assert!(entries(&net, &gen) > log);
    assert!(live(&a, 0).iter().any(|e| e.2 == "from bob") && live(&c, 0).iter().any(|e| e.2 == "from bob"));
    // Chat in every direction, a colour change, and a wall snapshot through the usual state all still work.
    for (p, t) in [(&mut a, "a says hi"), (&mut b, "b says hi"), (&mut c, "c says hi")] { p.c.send_text(&gen, t, None).unwrap(); }
    settle(&mut [&mut a, &mut b, &mut c]);
    for p in [&a, &b, &c] { for t in ["a says hi", "b says hi", "c says hi"] { assert!(has_text(p, &gen, t), "{} missing {t}", p.name); } }
    assert!(!notices(&b).iter().any(|n| n.contains("decrypt") || n.contains("out of sync")), "{:?}", notices(&b));
}

#[test]
fn only_current_members_can_post_or_poll_and_the_relay_stores_nothing() {
    let lim = commons_relay::live::LiveLimits { per_sec: 5, ..Default::default() };
    let net = Net::with_limits(commons_relay::Limits { live: lim, ..Default::default() });
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let mut z = Peer::new(&net, "zed"); // registered with the relay, in no room
    let (_cid, gen) = community_with(&net, &mut a, &mut [&mut b]);
    settle(&mut [&mut a, &mut b, &mut z]);
    let (api, kz) = z.c.net_handle().unwrap();
    let (api_a, ka) = a.c.net_handle().unwrap();
    let blob = vec![9u8; 200];
    // A stranger: 403 to post, and a poll about the room answers nothing about it.
    let e = api.live_post(&kz, &gen, &[blob.clone()]).unwrap_err();
    assert_eq!(commons_core::transport::relay_status(&e), Some(403));
    let mut req = LivePollReq::default();
    req.cursors.insert(gen.clone(), 0);
    let r = api.live_poll(&kz, &req).unwrap();
    assert!(r.groups.is_empty(), "a non-member is told nothing about the room, not even that it exists");
    // A member can post and poll; the poller gets what others posted and not its own.
    let ok = api_a.live_post(&ka, &gen, &[blob.clone(), blob.clone()]).unwrap();
    assert_eq!(ok.accepted, 2);
    let (api_b, kb) = b.c.net_handle().unwrap();
    let first = api_b.live_poll(&kb, &LivePollReq { cursors: [(gen.clone(), 0)].into(), ..Default::default() }).unwrap();
    assert_eq!(first.groups[&gen].events.len(), 0, "a poll from another run of the relay (empty boot) gets the position, not the buffer");
    api_a.live_post(&ka, &gen, &[blob.clone()]).unwrap();
    let again = api_b.live_poll(&kb, &LivePollReq { boot: first.boot.clone(), cursors: [(gen.clone(), first.groups[&gen].next)].into(), ..Default::default() }).unwrap();
    assert_eq!(again.groups[&gen].events.len(), 1);
    assert_eq!(again.groups[&gen].events[0].from, a.c.my_device());
    let own = api_a.live_poll(&ka, &LivePollReq { boot: first.boot.clone(), cursors: [(gen.clone(), 0)].into(), ..Default::default() }).unwrap();
    assert!(own.groups[&gen].events.is_empty(), "no echo of the caller's own device");
    // Limits: too big, too many per second, and a bad group id.
    let e = api_a.live_post(&ka, &gen, &[vec![1u8; LIVE_MAX_BLOB + 1]]).unwrap_err();
    assert_eq!(commons_core::transport::relay_status(&e), Some(413));
    std::thread::sleep(std::time::Duration::from_millis(1100)); // a fresh second
    let mut taken = 0;
    for _ in 0..4 { if api_a.live_post(&ka, &gen, &vec![blob.clone(); 3]).is_ok() { taken += 1; } }
    assert!(taken >= 1 && taken < 4, "the per-second limit cut in ({taken} of 4 posts went)");
    let e = api_a.live_post(&ka, &gen, &vec![blob.clone(); LIVE_POST_MAX + 1]).unwrap_err();
    assert_eq!(commons_core::transport::relay_status(&e), Some(400));
    // Nothing of it is in the database: the log and the usage counters did not move for any of this.
    let c = rusqlite::Connection::open_with_flags(net.db_path(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let tables: Vec<String> = c.prepare("SELECT name FROM sqlite_master WHERE type='table'").unwrap().query_map([], |r| r.get(0)).unwrap().map(|x| x.unwrap()).collect();
    assert!(!tables.iter().any(|t| t.contains("live")), "{tables:?}");
    assert!(!relay_bytes(&net).windows(200).any(|w| w == blob.as_slice()), "the blob never reached the disk");
}

#[test]
fn a_removed_member_stops_getting_news_and_cannot_post() {
    let net = Net::new();
    let (mut a, mut b, mut c, cid, gen) = trio(&net);
    a.c.acl_delay_secs = 0;
    a.c.live_send(&gen, "warm", "1").unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    a.c.kick(&cid, &b.acct()).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    settle(&mut [&mut a, &mut b, &mut c]);
    let seen_c = c.c.live_poll(0, None).last().map_or(0, |e| e.id);
    // Bob's client no longer sends in the room, and what he forces at the relay is refused.
    assert!(!b.c.live_send(&gen, "cursor", "x").unwrap_or(false));
    let (api, kb) = b.c.net_handle().unwrap();
    let e = api.live_post(&kb, &gen, &[vec![1u8; 100]]).unwrap_err();
    assert_eq!(commons_core::transport::relay_status(&e), Some(403), "{e}");
    let r = api.live_poll(&kb, &LivePollReq { cursors: [(gen.clone(), 0)].into(), ..Default::default() }).unwrap();
    assert!(r.groups.is_empty());
    // The others carry on, under the new epoch.
    a.c.live_send(&gen, "cursor", "after").unwrap();
    settle(&mut [&mut a, &mut c]);
    assert!(live(&c, seen_c).iter().any(|e| e.2 == "after"));
}

#[test]
fn events_keep_flowing_across_a_commit() {
    let net = Net::new();
    let (mut a, mut b, mut c, cid, gen) = trio(&net);
    a.c.live_send(&gen, "warm", "1").unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    let seen = b.c.live_poll(0, None).last().map_or(0, |e| e.id);
    // Carol leaves: a commit, so a new epoch and a new live key.
    c.c.leave(&cid).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    std::thread::sleep(std::time::Duration::from_millis(300)); // key cache is checked every quarter second
    a.c.live_send(&gen, "cursor", "new epoch").unwrap();
    settle(&mut [&mut a, &mut b]);
    assert!(live(&b, seen).iter().any(|e| e.2 == "new epoch"));
    assert!(c.c.live_poll(0, None).iter().all(|e| e.data != "new epoch"));
}

// ---------------------------------------------------------------------------------------------- latency (run by hand)
/// Two real apps (with their background threads) on a relay; how long from `live_send` on one to `live_poll` showing it on the other.
fn latency_ms(net: &Net, samples: usize, gap_ms: u64) -> Vec<f64> {
    use commons_core::api::App;
    use serde_json::json;
    let mk = |name: &str| {
        let dir = tempfile::tempdir().unwrap();
        let app = App::with_kdf(dir.path().to_path_buf(), &net.url(), commons_core::vault::KdfParams::TEST);
        app.call("create_account", json!({"name": name, "passphrase": "correct horse battery", "relay": net.url()})).unwrap();
        app.start_background();
        (app, dir)
    };
    let (a, _da) = mk("A");
    let (b, _db) = mk("B");
    let cid = a.call("create_community", json!({"name": "club"})).unwrap()["id"].as_str().unwrap().to_string();
    let gid = a.call("snapshot", json!({})).unwrap()["communities"][0]["channels"][0]["gid"].as_str().unwrap().to_string();
    let link = a.call("create_invite", json!({"cid": cid, "uses": 1})).unwrap()["link"].as_str().unwrap().to_string();
    b.call("join", json!({"link": link})).unwrap();
    let t = std::time::Instant::now();
    loop {
        let s = b.call("snapshot", json!({})).unwrap();
        let ch = s["communities"].as_array().and_then(|c| c.iter().find(|c| c["id"] == cid.as_str())).and_then(|c| c["channels"].as_array().and_then(|x| x.first().cloned()));
        if let Some(ch) = ch { if ch["joined"] == true { break; } let _ = b.call("join_channel", json!({"gid": gid})); }
        assert!(t.elapsed().as_secs() < 60, "b never joined");
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    std::thread::sleep(std::time::Duration::from_secs(3)); // both settled, the channel known
    let mut out = vec![];
    let mut since = 0u64;
    for i in 0..samples {
        let tag = format!("{{\"i\":{i}}}");
        let t0 = std::time::Instant::now();
        let sent = a.call("live_send", json!({"gid": gid, "kind": "tile", "data": tag})).unwrap();
        if sent["sent"] != true { std::thread::sleep(std::time::Duration::from_millis(gap_ms)); continue; }
        loop {
            let r = b.call("live_poll", json!({"gid": gid, "since": since})).unwrap();
            if let Some(e) = r["events"].as_array().and_then(|v| v.iter().find(|e| e["data"] == tag.as_str())) { since = e["id"].as_u64().unwrap(); out.push(t0.elapsed().as_secs_f64() * 1000.0); break; }
            if t0.elapsed().as_secs() > 8 { break; }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        std::thread::sleep(std::time::Duration::from_millis(gap_ms));
    }
    a.shutdown(); b.shutdown();
    out
}
fn summary(mut v: Vec<f64>) -> String {
    if v.is_empty() { return "no samples".into(); }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let q = |p: f64| v[((v.len() - 1) as f64 * p) as usize];
    format!("n={} min={:.0}ms median={:.0}ms p90={:.0}ms max={:.0}ms", v.len(), v[0], q(0.5), q(0.9), v[v.len() - 1])
}
#[test]
#[ignore = "a measurement, run by hand with --ignored --nocapture"]
fn measure_event_latency_new_path_against_the_log() {
    let new = latency_ms(&Net::new(), 40, 350);
    println!("LATENCY new unstored channel : {}", summary(new.clone()));
    let old = latency_ms(&old_relay(), 40, 350);
    println!("LATENCY old path through log : {}", summary(old.clone()));
    assert!(!new.is_empty() && !old.is_empty());
}
