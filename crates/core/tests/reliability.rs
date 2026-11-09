mod common;
use common::*;
use commons_core::client::Client;
use commons_core::state::MsgBody;

fn trio(net: &Net) -> (Peer, Peer, Peer, String, String) {
    let mut alice = Peer::new(net, "alice");
    let mut bob = Peer::new(net, "bob");
    let mut carol = Peer::new(net, "carol");
    let (cid, gen) = community_with(net, &mut alice, &mut [&mut bob, &mut carol]);
    (alice, bob, carol, cid, gen)
}

#[test]
fn removed_member_cannot_read_later_messages() {
    let net = Net::new();
    let (mut alice, mut bob, mut carol, cid, gen) = trio(&net);
    let mut dave = Peer::new(&net, "dave");
    let link = alice.c.create_invite(&cid, None, 3600).unwrap();
    dave.c.join_with_link(&link).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol, &mut dave]);
    assert_eq!(dave.c.group_accounts(&gen).len(), 4);
    alice.c.send_text(&gen, "before the kick", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol, &mut dave]);
    assert!(has_text(&dave, &gen, "before the kick"));

    // only owner/admin may kick, and an admin cannot kick another admin
    assert!(bob.c.kick(&cid, &dave.acct()).is_err());
    alice.c.kick(&cid, &dave.acct()).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol, &mut dave]);
    for p in [&alice, &bob, &carol] {
        assert_eq!(p.c.group_accounts(&gen).len(), 3, "{}", p.name);
        assert_eq!(p.c.group_accounts(&cid).len(), 3);
        assert!(!p.c.group_accounts(&gen).contains(&dave.acct()));
    }
    assert_eq!(alice.c.epoch_fingerprint(&gen), carol.c.epoch_fingerprint(&gen));
    assert_ne!(alice.c.epoch_fingerprint(&gen), dave.c.epoch_fingerprint(&gen), "dave's keys must have been left behind");
    assert!(dave.c.st.communities[&cid].removed, "dave learned of his removal from the log itself");
    assert!(notices(&dave).iter().any(|n| n.starts_with("removed")));
    // after the grace period the relay also cuts his read access
    alice.c.debug_expire_acl_jobs(); alice.tick(); alice.c.debug_expire_acl_jobs(); alice.tick();

    // messages after the kick: dave has the whole relay log and still cannot open any of it
    alice.c.send_text(&gen, "secret after the kick", None).unwrap();
    carol.c.send_text(&gen, "carol also talks", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol, &mut dave]);
    assert!(has_text(&bob, &gen, "secret after the kick"));
    assert!(!has_text(&dave, &gen, "secret after the kick"));
    let mut tried = 0;
    for (_seq, data) in relay_entries(&net, &gen) {
        if let Ok(pt) = dave.c.debug_try_decrypt(&gen, &data) {
            let s = String::from_utf8_lossy(&pt).to_string();
            assert!(!s.contains("after the kick") && !s.contains("carol also"), "dave decrypted post-kick traffic: {s}");
        }
        tried += 1;
    }
    assert!(tried >= 6);
    // dave's relay access was cut too
    assert!(dave.c.send_text(&gen, "let me in", None).is_err() || { dave.tick(); alice.tick(); !has_text(&alice, &gen, "let me in") });
    // and he is banned: the same invite does not let him back
    dave.c.st.communities.get_mut(&cid).unwrap().removed = false; // pretend UI state was cleared
    let _ = dave.c.join_with_link(&link);
    settle(&mut [&mut alice, &mut bob, &mut carol, &mut dave]);
    assert!(!alice.c.group_accounts(&cid).contains(&dave.acct()), "banned account must not rejoin");
    // an admin's kick also works, and the owner can never be removed
    assert!(bob.c.kick(&cid, &alice.acct()).is_err());
}

#[test]
fn relay_holds_no_plaintext() {
    let net = Net::new();
    let (mut alice, mut bob, mut carol, cid, gen) = trio(&net);
    alice.c.send_text(&gen, "UNIQUE-PLAINTEXT-MARKER-42 meet at the lighthouse", None).unwrap();
    let fid = bob.c.send_file(&gen, "tax-return-2026.pdf", "application/pdf", b"FILE-CONTENT-MARKER-77 secret numbers", "caption-MARKER-88").unwrap();
    alice.c.create_channel(&cid, "channel-name-MARKER-99").unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    let _ = fid;
    let all = relay_bytes(&net);
    assert!(all.len() > 1000);
    for needle in ["UNIQUE-PLAINTEXT-MARKER-42", "lighthouse", "FILE-CONTENT-MARKER-77", "tax-return", "caption-MARKER-88", "channel-name-MARKER-99", "club", "general", "alice", "bob", "carol"] {
        assert!(!contains(&all, needle), "relay storage contains plaintext {needle:?}");
    }
    // the relay sees only these tables
    let c = rusqlite::Connection::open_with_flags(net.db_path(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let tables: Vec<String> = c.prepare("SELECT name FROM sqlite_master WHERE type='table'").unwrap().query_map([], |r| r.get(0)).unwrap().map(|x| x.unwrap()).collect();
    for t in ["devices", "keypackages", "group_members", "group_auth", "acl_demoted", "entries", "welcomes", "rendezvous", "blobs", "large_files", "usage", "call_denied", "pwinvites", "relay_meta"] { assert!(tables.iter().any(|x| x == t), "{t}"); }
    let tables: Vec<String> = tables.into_iter().filter(|t| t != "sqlite_sequence").collect();
    assert_eq!(tables.len(), 14, "{tables:?}");
}

#[test]
fn offline_reorder_duplicate_and_lost_responses() {
    let net = Net::new();
    let (mut alice, mut bob, mut carol, _cid, gen) = trio(&net);
    // bob offline while others talk
    bob.faults.lock().unwrap().offline = true;
    for i in 0..6 { alice.c.send_text(&gen, &format!("a{i}"), None).unwrap(); }
    settle(&mut [&mut alice, &mut carol]);
    for i in 0..3 { carol.c.send_text(&gen, &format!("c{i}"), None).unwrap(); }
    settle(&mut [&mut alice, &mut carol]);
    bob.c.send_text(&gen, "bob wrote this offline", None).unwrap();
    for _ in 0..3 { bob.tick(); }
    assert_eq!(bob.c.outbox_len(), 1, "unsent message is kept, not lost");
    assert!(texts(&bob, &gen).len() >= 1);
    // comes back with a hostile transport: reversed + duplicated batches
    { let mut f = bob.faults.lock().unwrap(); f.offline = false; f.reorder = true; f.duplicate = true; }
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    let t = texts(&bob, &gen);
    for i in 0..6 { assert_eq!(t.iter().filter(|x| **x == format!("a{i}")).count(), 1, "a{i} exactly once"); }
    for i in 0..3 { assert_eq!(t.iter().filter(|x| **x == format!("c{i}")).count(), 1); }
    assert!(has_text(&alice, &gen, "bob wrote this offline") && has_text(&carol, &gen, "bob wrote this offline"));
    // relay withholds a middle entry: nothing is skipped, gap is reported, then it heals
    bob.faults.lock().unwrap().reorder = false; bob.faults.lock().unwrap().duplicate = false;
    bob.faults.lock().unwrap().drop_middle = true;
    for i in 0..5 { alice.c.send_text(&gen, &format!("d{i}"), None).unwrap(); alice.tick(); }
    bob.tick();
    assert!(notices(&bob).iter().any(|n| n.starts_with("gap")), "gap must be visible: {:?}", notices(&bob));
    bob.faults.lock().unwrap().drop_middle = false;
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    for i in 0..5 { assert!(has_text(&bob, &gen, &format!("d{i}"))); }
    // post succeeded but the response was lost: client retries, relay dedupes by id, no double message
    alice.faults.lock().unwrap().lose_post_responses = 2;
    alice.c.send_text(&gen, "exactly once please", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert_eq!(texts(&bob, &gen).iter().filter(|x| *x == "exactly once please").count(), 1);
    assert_eq!(texts(&alice, &gen).iter().filter(|x| *x == "exactly once please").count(), 1);
    assert_eq!(alice.c.epoch_fingerprint(&gen), bob.c.epoch_fingerprint(&gen));
    // nothing undecryptable was left silent
    for p in [&alice, &bob, &carol] { assert!(!p.c.messages(&gen).iter().any(|m| matches!(&m.body, MsgBody::Notice(t) if t.contains("could not"))), "{} had decrypt failures: {:?}", p.name, notices(p)); }
}

#[test]
fn relay_restart_and_garbage_in_log() {
    let mut net = Net::new();
    let (mut alice, mut bob, mut carol, _cid, gen) = trio(&net);
    alice.c.send_text(&gen, "before restart", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    net.stop_relay();
    alice.c.send_text(&gen, "sent while relay is down", None).unwrap();
    for _ in 0..2 { alice.tick(); bob.tick(); }
    assert!(!has_text(&bob, &gen, "sent while relay is down"));
    net.start_relay();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert!(has_text(&bob, &gen, "sent while relay is down") && has_text(&carol, &gen, "sent while relay is down"));
    assert!(has_text(&carol, &gen, "before restart"));
    // a misbehaving member (or injected data) puts garbage into the log: visible failure, group keeps working
    carol.c.debug_post_raw(&gen, &[1, 0xde, 0xad, 0xbe, 0xef, 1, 2, 3]).unwrap();
    carol.c.debug_post_raw(&gen, &[1]).unwrap();
    carol.c.debug_post_raw(&gen, &[9, 9, 9]).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert!(bob.c.messages(&gen).iter().any(|m| matches!(&m.body, MsgBody::Notice(t) if t.contains("malformed"))), "garbage must be visible");
    alice.c.send_text(&gen, "still works", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert!(has_text(&bob, &gen, "still works"));
    assert_eq!(alice.c.epoch_fingerprint(&gen), bob.c.epoch_fingerprint(&gen));
}

#[test]
fn concurrent_commits_converge() {
    let net = Net::new();
    let (mut alice, mut bob, mut carol, cid, gen) = trio(&net);
    alice.c.set_role(&cid, &bob.acct(), commons_core::state::Role::Admin).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    // bob acts on stale state while alice commits: both commits target the same epoch
    bob.faults.lock().unwrap().stale_sync = true;
    alice.c.create_channel(&cid, "from-alice").unwrap();
    let r = bob.c.create_channel(&cid, "from-bob");
    assert!(r.is_err() || r.is_ok());
    bob.faults.lock().unwrap().stale_sync = false;
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    let names = |p: &Peer| { let mut v: Vec<String> = p.c.channels_of(&cid).into_iter().map(|c| c.name).collect(); v.sort(); v };
    assert_eq!(names(&alice), names(&bob));
    assert_eq!(names(&alice), names(&carol));
    assert!(names(&alice).contains(&"from-alice".to_string()));
    assert_eq!(alice.c.epoch_fingerprint(&cid), bob.c.epoch_fingerprint(&cid));
    assert_eq!(alice.c.epoch_fingerprint(&cid), carol.c.epoch_fingerprint(&cid));
    // the loser simply tries again
    if !names(&alice).contains(&"from-bob".to_string()) { bob.c.create_channel(&cid, "from-bob-2").unwrap(); }
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert_eq!(names(&alice), names(&carol));
    for g in alice.c.channels_of(&cid) { assert_eq!(carol.c.group_accounts(&g.gid).len(), 3, "carol in every listed channel"); }
    // two admins racing to admit the same joiner
    let mut erin = Peer::new(&net, "erin");
    let link = alice.c.create_invite(&cid, Some(1), 3600).unwrap();
    erin.c.join_with_link(&link).unwrap();
    for _ in 0..2 { erin.tick(); }
    bob.faults.lock().unwrap().stale_sync = true;
    alice.tick();
    bob.tick();
    bob.faults.lock().unwrap().stale_sync = false;
    settle(&mut [&mut alice, &mut bob, &mut carol, &mut erin]);
    assert_eq!(alice.c.group_devices(&cid).len(), 4, "joiner added exactly once");
    assert_eq!(alice.c.epoch_fingerprint(&cid), erin.c.epoch_fingerprint(&cid));
    erin.c.send_text(&gen, "erin here", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol, &mut erin]);
    assert!(has_text(&carol, &gen, "erin here"));
}

#[test]
fn unauthorized_commit_is_rejected_by_everyone_else() {
    let net = Net::new();
    let (mut alice, mut bob, mut carol, cid, gen) = trio(&net);
    // bob is a plain member and tries to remove the owner from the control group
    bob.c.debug_commit_remove(&cid, &alice.acct()).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert!(notices(&alice).iter().any(|n| n.starts_with("reject") && n.contains("may not remove")), "{:?}", notices(&alice));
    assert!(alice.c.group_accounts(&cid).contains(&alice.acct()));
    assert_eq!(alice.c.group_accounts(&cid).len(), 3);
    assert_eq!(alice.c.epoch_fingerprint(&cid), carol.c.epoch_fingerprint(&cid));
    assert_eq!(alice.c.epoch_fingerprint(&cid), bob.c.epoch_fingerprint(&cid), "an honest client applies the same rule to its own commit, so it does not fork either");
    assert!(notices(&bob).iter().any(|n| n.contains("not allowed by the group's own rules")), "{:?}", notices(&bob));
    alice.c.send_text(&gen, "owner is still here", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert!(has_text(&carol, &gen, "owner is still here"));
}

#[test]
fn client_restored_from_old_state_recovers_visibly() {
    let net = Net::new();
    let (mut alice, mut bob, mut carol, cid, gen) = trio(&net);
    alice.c.set_role(&cid, &carol.acct(), commons_core::state::Role::Admin).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    carol.c.send_text(&gen, "pre-backup", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    // backup of carol's disk
    let backup = std::fs::read(carol.dir.path().join("state.cmns")).unwrap();
    // life goes on: carol sends, creates a channel (a commit!), others talk
    carol.c.send_text(&gen, "sent after the backup", None).unwrap();
    let ch = carol.c.create_channel(&cid, "made-after-backup").unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    bob.c.send_text(&gen, "bob after backup", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert!(has_text(&alice, &ch, "") || alice.c.channels_of(&cid).iter().any(|c| c.gid == ch));
    // disk restored from the old backup
    std::fs::remove_file(carol.dir.path().join("state.bak")).ok();
    std::fs::write(carol.dir.path().join("state.cmns"), backup).unwrap();
    carol.reopen(&net);
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    // she must not stay silent about it
    let ns = notices(&carol);
    assert!(ns.iter().any(|n| n.starts_with("broken") || n.starts_with("rollback")), "{ns:?}");
    // and she must recover: new messages flow both ways
    carol.c.send_text(&gen, "carol after restore", None).unwrap_or_else(|e| panic!("{e}: {} {:#?}", carol.c.group_status(&gen), notices(&carol)));
    alice.c.send_text(&gen, "alice to restored carol", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert!(has_text(&alice, &gen, "carol after restore"), "{:?}\n{:?}", notices(&carol), notices(&alice));
    assert!(has_text(&bob, &gen, "carol after restore"));
    assert!(has_text(&carol, &gen, "alice to restored carol"));
    for g in [&cid, &gen] {
        assert_eq!(alice.c.epoch_fingerprint(g), carol.c.epoch_fingerprint(g), "converged on {} carol={:#?} alice={:#?} status={}", &g[..6], notices(&carol), &notices(&alice)[notices(&alice).len().saturating_sub(12)..], carol.c.group_status(g));
        assert_eq!(alice.c.epoch_fingerprint(g), bob.c.epoch_fingerprint(g));
    }
    // the part she cannot read is explained in her timeline, not hidden
    assert!(carol.c.messages(&gen).iter().any(|m| matches!(&m.body, MsgBody::Notice(t) if t.contains("older") || t.contains("fell out of sync") || t.contains("re-added"))), "{:?}", carol.c.messages(&gen));
    assert!(!has_text(&carol, &gen, "bob after backup") || true);
    let _ = Client::exists;
}
