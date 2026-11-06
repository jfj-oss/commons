//! One test per bug found in the tester's report. Each failed before its fix.
mod common;
use common::*;
use commons_core::client::*;
use commons_core::envelope::*;
use commons_core::state::*;
use commons_core::vault::{KdfParams, Vault};

fn owner_and(net: &Net, a: &mut Peer, b: &mut Peer) -> (String, String) {
    a.tick(); b.tick();
    let mut o = [&mut *b];
    community_with(net, a, &mut o)
}

// ---- item 3: leaving and coming back
#[test]
fn rejoin_after_leaving_works() {
    let net = Net::new();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (cid, gen) = owner_and(&net, &mut a, &mut b);
    b.c.leave(&cid).unwrap();
    settle(&mut [&mut a, &mut b]);
    assert!(b.c.st.communities[&cid].removed, "left: {:?}", notices(&b));
    let link = a.c.create_invite(&cid, Some(1), 3600).unwrap();
    b.c.join_with_link(&link).unwrap();
    settle(&mut [&mut a, &mut b]);
    assert!(!b.c.st.communities[&cid].removed, "back in: {:?}", notices(&b));
    assert!(b.c.channels_of(&cid).iter().all(|c| c.joined));
    b.c.send_text(&gen, "back again", None).unwrap();
    a.c.send_text(&gen, "welcome back", None).unwrap();
    settle(&mut [&mut a, &mut b]);
    assert!(has_text(&a, &gen, "back again") && has_text(&b, &gen, "welcome back"));
}

#[test]
fn a_leavers_admin_grant_does_not_come_back() {
    let net = Net::new();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (cid, _) = owner_and(&net, &mut a, &mut b);
    let bob = b.acct();
    a.c.set_role(&cid, &bob, Role::Admin).unwrap();
    settle(&mut [&mut a, &mut b]);
    assert_eq!(b.c.st.communities[&cid].role_of(&bob), Some(Role::Admin));
    b.c.leave(&cid).unwrap();
    settle(&mut [&mut a, &mut b]);
    assert!(!a.c.st.communities[&cid].admins.contains(&bob), "grant pruned with the removal");
    let link = a.c.create_invite(&cid, Some(1), 3600).unwrap();
    b.c.join_with_link(&link).unwrap();
    settle(&mut [&mut a, &mut b]);
    assert!(!b.c.st.communities[&cid].removed);
    assert_eq!(a.c.st.communities[&cid].role_of(&bob), Some(Role::Member));
    assert_eq!(b.c.st.communities[&cid].role_of(&bob), Some(Role::Member));
}

// ---- item 4: two processes, one data dir
#[test]
fn a_second_open_of_the_same_data_dir_is_refused() {
    let net = Net::new();
    let a = Peer::new(&net, "alice");
    let (t, _) = transport(&net.url());
    let e = Client::open(a.dir.path(), "pw", t).err().expect("second open must fail while the first is alive");
    assert!(format!("{e:#}").contains("already"), "{e:#}");
    let (t, _) = transport(&net.url());
    let dir = a.dir.path().to_path_buf();
    drop(a.c);
    Client::open(&dir, "pw", t).expect("lock is released when the client goes away");
}

// ---- item 5: uses:1 means one
#[test]
fn a_single_use_invite_admits_one_person() {
    let net = Net::new();
    let (mut a, mut b, mut c, mut d) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"), Peer::new(&net, "carol"), Peer::new(&net, "dave"));
    for p in [&mut a, &mut b, &mut c, &mut d] { p.tick(); }
    let cid = a.c.create_community("club").unwrap();
    let link = a.c.create_invite(&cid, Some(1), 3600).unwrap();
    for p in [&mut b, &mut c, &mut d] { p.c.join_with_link(&link).unwrap(); }
    settle(&mut [&mut a, &mut b, &mut c, &mut d]);
    assert_eq!(a.c.group_accounts(&cid).len(), 2, "owner + exactly one");
    assert!(a.c.st.communities[&cid].invites.is_empty());
}

// ---- item 6: hostile mime and channel ids never reach the UI
#[test]
fn hostile_mime_and_channel_ids_are_refused() {
    let id = "0123456789abcdef0123456789abcdef".to_string();
    let f = |mime: &str| Envelope::File { id: id.clone(), name: "a".into(), mime: mime.into(), size: 1, blob: id.clone(), key: "00".repeat(32), nonce: "00".repeat(12), caption: "".into(), reply: None, ts: 1 }.to_bytes();
    assert!(Envelope::parse(&f("image/png")).is_ok());
    assert!(Envelope::parse(&f("application/vnd.oasis.opendocument.text+xml")).is_ok());
    for bad in ["image/png\" onerror=\"x", "image/png;base64,AAAA", "IMAGE/PNG", "image", "/png", "image/", "a/b/c", "image/png ", ""] {
        assert!(Envelope::parse(&f(bad)).is_err(), "{bad:?}");
    }
    let tbl = |cid: &str| GroupExt::Control { name: "n".into(), owner: "a".repeat(64), nonce: id.clone(), admins: vec![], channels: vec![ChanMeta { id: cid.into(), name: "c".into() }], invites: vec![], banned: vec![], transfers: vec![] }.to_bytes();
    assert!(GroupExt::parse(&tbl("abcdef0123")).is_ok());
    assert!(GroupExt::parse(&tbl("\"><img src=x onerror=alert(1)>")).is_err());
}

#[test]
fn sending_cleans_a_bad_mime_instead_of_breaking_the_message() {
    let net = Net::new();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (_, gen) = owner_and(&net, &mut a, &mut b);
    let id = a.c.send_file(&gen, "x.bin", "image/png\" onerror=\"x", b"data", "").unwrap();
    settle(&mut [&mut a, &mut b]);
    let m = b.c.messages(&gen).into_iter().find(|m| m.id == id).expect("delivered");
    let MsgBody::File { mime, .. } = m.body else { panic!() };
    assert_eq!(mime, "application/octet-stream");
}

// ---- item 8: file names are limited in bytes
#[test]
fn long_multibyte_file_names_are_cut_on_a_char_boundary() {
    let net = Net::new();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (_, gen) = owner_and(&net, &mut a, &mut b);
    let long = format!("{}.txt", "日".repeat(200));
    let id = a.c.send_file(&gen, &long, "text/plain", b"cjk", "").unwrap();
    settle(&mut [&mut a, &mut b]);
    let m = b.c.messages(&gen).into_iter().find(|m| m.id == id).expect("receiver understood the file");
    let MsgBody::File { name, .. } = m.body else { panic!() };
    assert!(name.len() <= 200 && name.ends_with(".txt") && name.starts_with('日'), "{name}");
}

// ---- item 9: what we queue must be what receivers accept
#[test]
fn bad_edits_and_reactions_are_refused_before_queueing() {
    let net = Net::new();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (_, gen) = owner_and(&net, &mut a, &mut b);
    let id = a.c.send_text(&gen, "original", None).unwrap();
    settle(&mut [&mut a, &mut b]);
    let n = a.c.outbox_len();
    assert!(a.c.edit_message(&gen, &id, &"y".repeat(MAX_TEXT + 1)).is_err(), "oversize edit");
    assert!(a.c.edit_message(&gen, &id, "   ").is_err(), "empty edit");
    assert!(a.c.react(&gen, &id, &"x".repeat(40), true).is_err(), "long reaction");
    assert!(a.c.react(&gen, &id, "", true).is_err(), "empty reaction");
    assert!(a.c.react(&gen, "zz", "👍", true).is_err(), "garbage target");
    assert!(a.c.react(&gen, &"0".repeat(32), "👍", true).is_err(), "unknown target");
    assert!(a.c.send_text(&gen, "x", Some("not-an-id".into())).is_err(), "garbage reply");
    assert_eq!(a.c.outbox_len(), n, "nothing was queued");
    assert!(has_text(&a, &gen, "original"), "local copy untouched");
    a.c.react(&gen, &id, "👍", true).unwrap();
    settle(&mut [&mut a, &mut b]);
    assert_eq!(b.c.messages(&gen).iter().find(|m| m.id == id).unwrap().reactions.len(), 1);
}

// ---- item 10: names are checked before anything is created
#[test]
fn bad_names_create_nothing() {
    let net = Net::new();
    let mut a = Peer::new(&net, "alice");
    a.tick();
    let long = "N".repeat(100);
    for bad in ["", "   ", "Bad\u{7}", long.as_str(), "two\nlines"] {
        assert!(a.c.create_community(bad).is_err(), "{bad:?}");
    }
    assert!(a.c.st.communities.is_empty() && a.c.st.groups.is_empty(), "no zombie community");
    let cid = a.c.create_community("club").unwrap();
    let groups = a.c.st.groups.len();
    for bad in ["", "bad\u{7}name", long.as_str()] {
        assert!(a.c.create_channel(&cid, bad).is_err(), "{bad:?}");
        assert!(a.c.rename_community(&cid, bad).is_err(), "{bad:?}");
    }
    assert_eq!(a.c.st.groups.len(), groups, "no orphan channel group");
    assert_eq!(a.c.st.communities[&cid].name, "club");
    assert!(a.c.set_name("bo\rb").is_err() && a.c.set_name(&long).is_err());
}

// ---- item 11: damage is not a wrong passphrase
#[test]
fn a_damaged_state_file_falls_back_to_the_backup_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("state.cmns");
    let v = Vault::create(&p, "hunter2hunter2", KdfParams::TEST).unwrap();
    v.save(b"one").unwrap(); v.save(b"two").unwrap();
    let mut raw = std::fs::read(&p).unwrap();
    let n = raw.len() - 3; raw[n] ^= 1; // body damaged: authentication fails, same as a wrong key
    std::fs::write(&p, &raw).unwrap();
    assert!(matches!(Vault::open(&p, "wrong").err().unwrap().downcast_ref::<commons_core::vault::VaultError>(), Some(commons_core::vault::VaultError::WrongPassphrase)));
    let (_v, got) = Vault::open(&p, "hunter2hunter2").expect("right passphrase opens the backup");
    assert_eq!(got, b"one");
}

// ---- item 12: the relay lost us
#[test]
fn a_relay_that_forgot_us_is_noticed_and_we_register_again() {
    let mut net = Net::new();
    let mut a = Peer::new(&net, "alice");
    a.tick();
    net.stop_relay();
    std::fs::remove_dir_all(net.data.path().join("relay")).unwrap();
    net.start_relay();
    for _ in 0..3 { a.tick(); }
    assert!(notices(&a).iter().any(|n| n.starts_with("relay:") && n.contains("no longer knows")), "{:?}", notices(&a));
    assert!(a.c.st.registered);
    a.c.create_community("again").expect("works once registered again");
}
