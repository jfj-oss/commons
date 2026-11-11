//! History for a new device (docs/history-transfer.md): a freshly paired device asks the account's other devices, the person says
//! yes on one of them, and that device's rooms arrive sealed to the new device through the relay's inbox. Nobody else is involved,
//! the relay sees ciphertext, a stranger can neither ask nor send, and both sides resume after a restart.
mod common;
use common::*;
use commons_core::client::Client;
use commons_core::state::MsgBody;
use commons_core::vault::KdfParams;

fn peer_from(c: Client, dir: tempfile::TempDir, faults: std::sync::Arc<std::sync::Mutex<Faults>>, name: &str) -> Peer { Peer { name: name.into(), c, dir, faults } }

/// A new device of `old`'s account, paired the usual way; `preapprove` is the tick box on the approving screen.
fn pair(net: &Net, old: &mut Peer, others: &mut [&mut Peer], name: &str, preapprove: bool) -> Peer {
    let dir = tempfile::tempdir().unwrap();
    let (t, faults) = transport(&net.url());
    let (c2, code) = Client::create_for_pairing(dir.path(), "pw2", &net.url(), "", KdfParams::TEST, t).unwrap();
    let mut new = peer_from(c2, dir, faults, name);
    old.c.pairing_approve(&code).unwrap();
    if preapprove { old.c.history_preapprove(); }
    let mut all: Vec<&mut Peer> = vec![old, &mut new];
    for o in others.iter_mut() { all.push(o); }
    settle(&mut all);
    assert!(new.c.has_identity(), "paired");
    new
}
fn ids(p: &Peer, gid: &str) -> Vec<String> { p.c.message_log(gid).iter().map(|m| m.id.clone()).collect() }
fn inbox_rows(net: &Net) -> u64 {
    rusqlite::Connection::open_with_flags(net.db_path(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap().query_row("SELECT COUNT(*) FROM welcomes", [], |r| r.get(0)).unwrap()
}

#[test]
fn a_new_device_gets_the_history_after_the_yes_given_with_the_pairing_approval() {
    let net = Net::new();
    let (mut alice, mut bob) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (cid, gen) = community_with(&net, &mut alice, &mut [&mut bob]);
    let gen2 = alice.c.create_channel(&cid, "second").unwrap();
    settle(&mut [&mut alice, &mut bob]);
    for i in 0..30 { alice.c.send_text(&gen, &format!("line {i}"), None).unwrap(); }
    bob.c.send_text(&gen, "bob here", None).unwrap();
    let fid = alice.c.send_file(&gen, "pic.bin", "application/octet-stream", &[7u8; 2000], "a picture").unwrap();
    alice.c.send_text(&gen2, "in the second room", None).unwrap();
    settle(&mut [&mut alice, &mut bob]);
    bob.c.mark_read(&gen);
    for i in 0..3 { alice.c.send_text(&gen, &format!("unread {i}"), None).unwrap(); }
    settle(&mut [&mut alice, &mut bob]);
    assert_eq!(bob.c.unread(&gen), 3);
    let (rooms, msgs) = bob.c.history_summary();
    assert!(rooms == 2 && msgs >= 36, "{rooms} rooms, {msgs} messages");

    // bob approves his new laptop with "also send my history" ticked
    let mut bob2 = pair(&net, &mut bob, &mut [&mut alice], "bob2", true);
    settle(&mut [&mut alice, &mut bob, &mut bob2]);
    let h = bob2.c.st.history_in.as_ref().expect("history arrived");
    assert!(h.done > 0 && h.got.len() as u32 == h.n && h.omitted == 0, "{h:?}");
    assert_eq!(h.from, bob.c.my_device());
    assert_eq!(ids(&bob2, &gen), ids(&bob, &gen), "the same messages in the same order");
    assert_eq!(ids(&bob2, &gen2), ids(&bob, &gen2));
    assert!(has_text(&bob2, &gen, "line 0") && has_text(&bob2, &gen, "bob here") && has_text(&bob2, &gen, "unread 2") && has_text(&bob2, &gen2, "in the second room"));
    assert_eq!(bob2.c.unread(&gen), 3, "the unread count travelled: {:?}", bob2.c.st.chans[&gen].read_upto);
    let f = bob2.c.messages(&gen).into_iter().find(|m| m.id == fid).expect("the file message is there");
    assert!(matches!(&f.body, MsgBody::File { name, key, .. } if name == "pic.bin" && key.len() == 64), "the file's key came along, so the blob can be fetched");
    assert_eq!(bob2.c.messages(&gen).iter().find(|m| m.sender == bob.acct()).unwrap().status, "sent", "bob's own lines read as his");
    let transferred = ids(&bob, &gen).len() + ids(&bob, &gen2).len();
    // both devices keep working in the rooms afterwards
    alice.c.send_text(&gen, "after the transfer", None).unwrap();
    bob2.c.send_text(&gen, "from the laptop", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut bob2]);
    assert!(has_text(&bob2, &gen, "after the transfer") && has_text(&alice, &gen, "from the laptop") && has_text(&bob, &gen, "from the laptop"));
    assert!(bob2.c.message_log(&gen).last().unwrap().id != ids(&bob2, &gen)[0], "new messages come after the history");
    // nobody else noticed anything, the inbox is empty again, the sender's queue is drained, and the status reads done
    assert!(!notices(&alice).iter().any(|n| n.contains("history")), "{:?}", notices(&alice));
    assert_eq!(inbox_rows(&net), 0);
    assert!(bob.c.st.history_out.is_empty() && bob.c.st.history_requests.is_empty());
    let st = bob2.c.history_status();
    assert_eq!(st["incoming"]["state"], "done");
    assert_eq!(st["incoming"]["msgs"].as_u64().unwrap() as usize, transferred);
    let sb = bob.c.history_status();
    assert_eq!(sb["sending"][0]["done"], true);
    bob2.c.history_dismiss().unwrap();
    assert_eq!(bob2.c.history_status()["incoming"]["state"], "none");
}

#[test]
fn without_the_yes_the_request_waits_and_strangers_can_neither_ask_nor_send() {
    let net = Net::new();
    let (mut alice, mut bob, mut carol) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"), Peer::new(&net, "carol"));
    let (_cid, gen) = community_with(&net, &mut alice, &mut [&mut bob, &mut carol]);
    for i in 0..200 { alice.c.send_text(&gen, &format!("a fairly long line of text so that the history fills several chunks, number {i}"), None).unwrap(); }
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    bob.c.debug_history_chunk_bytes(16 * 1024);
    let mut bob2 = pair(&net, &mut bob, &mut [&mut alice, &mut carol], "bob2", false);
    settle(&mut [&mut alice, &mut bob, &mut bob2, &mut carol]);
    // the request sits with bob; the new device has nothing and says it is asking
    let dev = bob2.c.my_device();
    assert!(bob.c.st.history_requests.contains_key(&dev), "{:?}", notices(&bob));
    assert!(bob2.c.st.history_in.is_none() && bob2.c.messages(&gen).is_empty());
    assert_eq!(bob2.c.history_status()["incoming"]["state"], "asking");
    assert_eq!(bob.c.history_status()["requests"][0]["device"], dev);
    assert!(notices(&bob).iter().any(|n| n.contains("asks for this device's message history")));
    assert!(!notices(&alice).iter().any(|n| n.contains("history")) && !notices(&carol).iter().any(|n| n.contains("history")));

    // carol (another account) asks bob, and sends bob2 a chunk: both are ignored (her certificate is not bob's account's)
    let forged_req = carol.c.history_request_bytes().unwrap();
    carol.c.debug_post_inbox(&bob.c.my_device(), &forged_req).unwrap();
    let mut fake = b"HIST1".to_vec();
    fake.extend(serde_json::to_vec(&serde_json::json!({"tid": "00".repeat(16), "i": 0, "n": 1, "cert": carol.c.cert_public(), "blob": "AAAA", "sig": "00".repeat(64)})).unwrap());
    carol.c.debug_post_inbox(&dev, &fake).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut bob2, &mut carol]);
    assert_eq!(bob.c.st.history_requests.len(), 1, "carol's request was not taken");
    assert!(bob2.c.st.history_in.is_none());
    assert!(notices(&bob).iter().any(|n| n.contains("ignored a history request from another account's device")), "{:?}", notices(&bob));
    assert!(notices(&bob2).iter().any(|n| n.contains("ignored a history chunk from another account's device")), "{:?}", notices(&bob2));

    // declining leaves the new device waiting; asking again (a restart) brings the request back
    bob.c.history_decline(&dev).unwrap();
    assert!(bob.c.st.history_requests.is_empty());
    bob2.c.st.history_asked = 0;
    settle(&mut [&mut alice, &mut bob, &mut bob2, &mut carol]);
    assert!(bob.c.st.history_requests.contains_key(&dev));

    // the yes: chunks are built sealed (no plaintext in what the relay will carry), several of them, and arrive in order
    let r = bob.c.history_send(&dev).unwrap();
    let n = r["chunks"].as_u64().unwrap();
    assert!(n >= 3, "{r}");
    for h in &bob.c.st.history_out {
        let item = commons_proto::unb64(&h.data).unwrap();
        assert!(item.starts_with(b"HIST1"));
        let text = String::from_utf8_lossy(&item);
        assert!(!text.contains("fairly long line") && !text.contains(&gen), "the relay's copy shows no message and no room: {}", &text[..80]);
    }
    assert!(bob.c.history_send(&dev).is_err(), "the request is spent");
    settle(&mut [&mut alice, &mut bob, &mut bob2, &mut carol]);
    let h = bob2.c.st.history_in.as_ref().unwrap();
    assert!(h.done > 0 && h.n as u64 == n, "{h:?}");
    assert_eq!(ids(&bob2, &gen), ids(&bob, &gen));
    assert_eq!(inbox_rows(&net), 0);
}

#[test]
fn a_transfer_resumes_after_the_new_device_restarts_and_stops_for_a_revoked_one() {
    let net = Net::new();
    let (mut alice, mut bob) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (_cid, gen) = community_with(&net, &mut alice, &mut [&mut bob]);
    for i in 0..300 { alice.c.send_text(&gen, &format!("a fairly long line of text so that the history fills several chunks, number {i}"), None).unwrap(); }
    settle(&mut [&mut alice, &mut bob]);
    bob.c.debug_history_chunk_bytes(16 * 1024);
    let mut bob2 = pair(&net, &mut bob, &mut [&mut alice], "bob2", false);
    settle(&mut [&mut alice, &mut bob, &mut bob2]);
    let dev = bob2.c.my_device();
    let n = bob.c.history_send(&dev).unwrap()["chunks"].as_u64().unwrap() as u32;
    assert!(n >= 6, "{n} chunks");
    // one round on each side: at most four chunks are in flight, so the new device has some, not all
    bob.tick(); bob2.tick();
    let got = bob2.c.st.history_in.as_ref().map(|h| h.got.len() as u32).unwrap_or(0);
    assert!(got > 0 && got < n, "got {got} of {n}");
    assert_eq!(bob2.c.history_status()["incoming"]["state"], "receiving");
    // the new device restarts: what it had is kept, the rest follows
    { let (t, f) = transport(&net.url()); bob2.c.debug_release_lock(); bob2.c = Client::open(bob2.dir.path(), "pw2", t).unwrap(); impatient(&mut bob2.c); bob2.faults = f; }
    assert_eq!(bob2.c.st.history_in.as_ref().unwrap().got.len() as u32, got);
    settle(&mut [&mut alice, &mut bob, &mut bob2]);
    let h = bob2.c.st.history_in.as_ref().unwrap();
    assert!(h.done > 0 && h.got.len() as u32 == n, "{h:?}");
    assert_eq!(ids(&bob2, &gen), ids(&bob, &gen));
    assert!(bob.c.st.history_out.is_empty());

    // a third device asks, bob says yes, then revokes it before the chunks are through: nothing more goes to it
    let mut bob3 = pair(&net, &mut bob, &mut [&mut alice], "bob3", false);
    settle(&mut [&mut alice, &mut bob, &mut bob3]);
    let dev3 = bob3.c.my_device();
    bob.c.history_send(&dev3).unwrap();
    bob.c.revoke_device(&dev3).unwrap();
    settle(&mut [&mut alice, &mut bob]);
    assert!(bob.c.st.history_out.is_empty(), "the queue for a revoked device is dropped: {}", bob.c.st.history_out.len());
    assert!(!bob.c.st.welcome_out.iter().any(|w| w.device == dev3), "nothing waits for it in the inbox queue");
}
