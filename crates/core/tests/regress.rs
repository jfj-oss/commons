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
    let tbl = |cid: &str| GroupExt::Control { name: "n".into(), owner: "a".repeat(64), nonce: id.clone(), admins: vec![], channels: vec![ChanMeta { id: cid.into(), name: "c".into() }], invites: vec![], banned: vec![], transfers: vec![], consents: vec![] }.to_bytes();
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

// ---- items 5, 14: a join that will not happen is reported to the joiner
#[test]
fn refused_joiners_find_out() {
    let net = Net::new();
    let (mut a, mut b, mut c) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"), Peer::new(&net, "carol"));
    for p in [&mut a, &mut b, &mut c] { p.tick(); }
    let cid = a.c.create_community("club").unwrap();
    let link = a.c.create_invite(&cid, Some(1), 3600).unwrap();
    b.c.join_with_link(&link).unwrap();
    c.c.join_with_link(&link).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    let (inn, out) = if a.c.group_accounts(&cid).contains(&b.acct()) { (&b, &c) } else { (&c, &b) };
    assert!(inn.c.st.join_refused.is_empty());
    assert!(out.c.st.joining.is_empty(), "no longer waiting");
    assert!(out.c.st.join_refused[&cid].reason.contains("no longer valid"));
    assert!(notices(out).iter().any(|n| n.starts_with("reject")), "and it shows");
    // kicked, then trying a fresh link
    let bob_in = a.c.group_accounts(&cid).contains(&b.acct());
    let (k, x) = if bob_in { (&mut b, &mut c) } else { (&mut c, &mut b) };
    let t = k.acct();
    a.c.kick(&cid, &t).unwrap();
    settle(&mut [&mut a, &mut *k, &mut *x]);
    let link = a.c.create_invite(&cid, Some(1), 3600).unwrap();
    k.c.join_with_link(&link).unwrap();
    settle(&mut [&mut a, &mut *k, &mut *x]);
    assert!(k.c.st.join_refused[&cid].reason.contains("banned"), "{:?}", notices(k));
    k.c.cancel_join(&cid).unwrap();
    assert!(k.c.st.join_refused.is_empty());
}

#[test]
fn a_stranger_cannot_call_off_a_join() {
    let (founder, device) = ("a".repeat(64), "b".repeat(64));
    let stranger = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let dev = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    use ed25519_dalek::Signer;
    let cert = commons_proto::DeviceCert::issue(&stranger, &dev.verifying_key().to_bytes());
    let sig = hex::encode(dev.sign(&join_reply_msg("abcdef0123", &device, JOIN_BANNED)).to_bytes());
    let r = JoinReply { community: "abcdef0123".into(), reason: JOIN_BANNED.into(), cert, sig, grant: None, transfers: vec![] };
    assert!(r.verify(&founder, &device).is_err());
}

#[test]
fn an_unanswered_join_can_be_called_off() {
    let net = Net::new();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    a.tick(); b.tick();
    let cid = a.c.create_community("club").unwrap();
    let link = a.c.create_invite(&cid, Some(1), 3600).unwrap();
    b.c.join_with_link(&link).unwrap();
    b.tick();
    assert!(b.c.st.join_sent.contains(&cid));
    b.c.cancel_join(&cid).unwrap();
    assert!(b.c.st.joining.is_empty() && b.c.st.join_sent.is_empty());
}

// ---- item 14: a pairing nobody finished
#[test]
fn an_abandoned_pairing_can_be_resumed_or_given_up() {
    let net = Net::new();
    let dir = tempfile::tempdir().unwrap();
    let mut a = Peer::new(&net, "alice");
    a.tick();
    let (mut c, code) = Client::create_for_pairing(dir.path(), "pw2", &net.url(), "", KdfParams::TEST, transport(&net.url()).0).unwrap();
    c.debug_release_lock(); drop(c);
    let mut c = Client::open(dir.path(), "pw2", transport(&net.url()).0).unwrap();
    assert!(!c.has_identity());
    assert_eq!(c.pairing_code().unwrap(), code, "the same code comes back after a restart");
    // junk at the mailbox is ignored and the wait goes on
    let raw = code.strip_prefix("commons-pair:").unwrap();
    let j: serde_json::Value = serde_json::from_slice(&commons_proto::unb64(raw).unwrap()).unwrap();
    let rid = j["rid"].as_str().unwrap();
    commons_core::transport::Api { t: transport(&net.url()).0 }.rv_put(&a.c.net_handle().unwrap().1, rid, "a", b"not sealed").unwrap();
    assert!(!c.pairing_poll().unwrap());
    assert!(c.pairing_code().is_some());
    c.abandon_pairing().unwrap();
    assert!(!Client::exists(dir.path()));
}

// ---- item 12 and low: why the relay will not talk to us
#[test]
fn a_revoked_device_is_told_and_a_new_device_is_pointed_out() {
    let net = Net::new();
    let mut a = Peer::new(&net, "alice");
    a.tick();
    let key = a.c.recovery_key().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut c2 = Client::create_from_recovery(dir.path(), "pw", &net.url(), "alice", &key, KdfParams::TEST, transport(&net.url()).0).unwrap();
    impatient(&mut c2);
    for _ in 0..2 { a.tick(); }
    assert!(notices(&a).iter().any(|n| n.contains("A new device")), "{:?}", notices(&a));
    let dev = c2.my_device();
    a.c.revoke_device(&dev).unwrap();
    assert!(notices(&a).iter().any(|n| n.contains("still holds your account key")));
    let _ = c2.tick(0);
    assert!(c2.st.self_revoked);
    let cid = a.c.create_community("club").unwrap();
    let gen = a.c.channels_of(&cid)[0].gid.clone();
    assert!(a.c.send_text(&gen, "still here", None).is_ok());
    c2.st.groups.insert(gen.clone(), GroupState::new(&gen, &cid, Kind::Channel));
    assert!(c2.send_text(&gen, "x", None).unwrap_err().to_string().contains("revoked"));
    assert!(c2.st.notices.iter().any(|n| n.kind == "revoked"));
}

#[test]
fn a_wrong_clock_is_named() {
    let net = Net::new();
    let mut a = Peer::new(&net, "alice");
    a.tick();
    a.faults.lock().unwrap().skewed_clock = true;
    a.tick();
    assert!(notices(&a).iter().any(|n| n.starts_with("clock:") && n.contains("50 minutes")), "{:?}", notices(&a));
    assert_eq!(a.c.net.state, NetState::Offline);
    a.faults.lock().unwrap().skewed_clock = false;
    a.tick();
    assert_eq!(a.c.net.state, NetState::Online);
}

// ---- item 15
#[test]
fn relay_addresses_are_checked_before_an_account_exists() {
    use commons_core::api::*;
    assert_eq!(clean_relay_url(" https://chat.example.org/ ").unwrap(), "https://chat.example.org");
    assert_eq!(clean_relay_url("http://127.0.0.1:8787").unwrap(), "http://127.0.0.1:8787");
    assert_eq!(clean_relay_url("https://machine.tailnet-name.ts.net").unwrap(), "https://machine.tailnet-name.ts.net");
    for bad in ["ftp://x", "chat.example.org", "http://", "http://a b", "http://u:p@host", "https://x?y=1", ""] { assert!(clean_relay_url(bad).is_err(), "{bad}"); }
    let dir = tempfile::tempdir().unwrap();
    let app = App::with_kdf(dir.path().into(), "http://127.0.0.1:9", KdfParams::TEST);
    let mk = |relay: &str| app.call("create_account", serde_json::json!({"name": "a", "passphrase": "passphrase1", "relay": relay}));
    assert!(mk("ftp://x").unwrap_err().contains("not a relay address"));
    assert!(mk("http://127.0.0.1:9").unwrap_err().contains("Could not reach"));
    assert!(!Client::exists(dir.path()), "nothing was created");
    let net = Net::new();
    mk(&net.url()).unwrap();
    assert!(Client::exists(dir.path()));
}

// ---- item 6: the dev bridge
#[test]
fn the_dev_bridge_only_answers_its_own_page() {
    use commons_core::api::bridge_request_ok as ok;
    assert!(ok(Some("127.0.0.1:7001"), None, 7001));
    assert!(ok(Some("localhost:7001"), Some("http://localhost:7001"), 7001));
    assert!(!ok(Some("evil.example:7001"), None, 7001), "DNS rebinding");
    assert!(!ok(Some("127.0.0.1:7001"), Some("https://evil.example"), 7001), "cross-site post");
    assert!(!ok(None, None, 7001));
}

// ---- item 13: a relay that hangs does not hang the app
#[test]
fn the_ui_keeps_working_while_the_relay_hangs() {
    use commons_core::api::*;
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};
    let hole = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", hole.local_addr().unwrap());
    std::thread::spawn(move || {
        let mut held = vec![];
        for s in hole.incoming() {
            let Ok(mut s) = s else { continue };
            let mut b = [0u8; 2048];
            let n = s.read(&mut b).unwrap_or(0);
            if b[..n].starts_with(b"GET /health") { let _ = s.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok"); } else { held.push(s); } // answers nothing, ever
        }
    });
    let net = Net::new();
    let dir = tempfile::tempdir().unwrap();
    let app = App::with_kdf(dir.path().into(), &net.url(), KdfParams::TEST);
    app.call("create_account", serde_json::json!({"name": "a", "passphrase": "passphrase1"})).unwrap();
    app.call("create_community", serde_json::json!({"name": "club"})).unwrap();
    let snap = app.call("snapshot", serde_json::json!({})).unwrap();
    let gid = snap["communities"][0]["channels"][0]["gid"].as_str().unwrap().to_string();
    app.call("set_relay", serde_json::json!({"relay": url})).unwrap();
    app.start_background();
    std::thread::sleep(Duration::from_secs(3)); // the first round is now stuck on the relay
    let t = Instant::now();
    let s = app.call("snapshot", serde_json::json!({})).unwrap();
    app.call("send_text", serde_json::json!({"gid": gid, "text": "while the relay hangs"})).unwrap();
    let m = app.call("messages", serde_json::json!({"gid": gid})).unwrap();
    assert!(t.elapsed() < Duration::from_secs(1), "took {:?}", t.elapsed());
    assert_eq!(s["net"], "connecting", "and the UI can say so");
    assert_eq!(m["messages"].as_array().unwrap().len(), 1);
    app.shutdown();
}

// ---- performance: pages of a long history
#[test]
fn messages_come_in_pages() {
    use commons_core::api::*;
    let net = Net::new();
    let dir = tempfile::tempdir().unwrap();
    let app = App::with_kdf(dir.path().into(), &net.url(), KdfParams::TEST);
    app.call("create_account", serde_json::json!({"name": "a", "passphrase": "passphrase1"})).unwrap();
    app.call("create_community", serde_json::json!({"name": "club"})).unwrap();
    let gid = app.call("snapshot", serde_json::json!({})).unwrap()["communities"][0]["channels"][0]["gid"].as_str().unwrap().to_string();
    let mut ids = vec![];
    for i in 0..60 { ids.push(app.call("send_text", serde_json::json!({"gid": gid, "text": format!("m{i}")})).unwrap()["id"].as_str().unwrap().to_string()); }
    let p = app.call("messages", serde_json::json!({"gid": gid, "limit": 5})).unwrap();
    assert_eq!(p["messages"].as_array().unwrap().len(), 5);
    assert_eq!(p["more"], true);
    assert_eq!(p["messages"][4]["text"], "m59", "the newest");
    let p = app.call("messages", serde_json::json!({"gid": gid, "limit": 5, "around": ids[30]})).unwrap();
    assert!(p["messages"].as_array().unwrap().iter().any(|m| m["id"] == ids[30].as_str()), "a search result is always in the page");
    // and a big file reads back in pieces
    let data = vec![7u8; (1 << 20) + 5];
    let id = app.call("send_file", serde_json::json!({"gid": gid, "name": "../../x.bin", "mime": "application/x", "data": commons_proto::b64(&data), "caption": ""})).unwrap()["id"].as_str().unwrap().to_string();
    let f = app.call("file_open", serde_json::json!({"gid": gid, "id": id})).unwrap();
    assert_eq!(f["size"], data.len());
    assert_eq!(f["save_name"], "_.._x.bin", "no path survives into a save dialog");
    let mut got = vec![];
    while got.len() < data.len() { got.extend(commons_proto::unb64(app.call("file_read", serde_json::json!({"gid": gid, "id": id, "offset": got.len()})).unwrap()["data"].as_str().unwrap()).unwrap()); }
    assert_eq!(got, data);
}

// ---- suspected: all real
#[test]
fn ownership_can_be_handed_over() {
    let net = Net::new();
    let (mut a, mut b, mut c, mut d) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"), Peer::new(&net, "carol"), Peer::new(&net, "dave"));
    for p in [&mut a, &mut b, &mut c, &mut d] { p.tick(); }
    let (cid, gen) = community_with(&net, &mut a, &mut [&mut b, &mut c]);
    a.c.set_role(&cid, &c.acct(), Role::Admin).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    assert!(a.c.leave(&cid).is_err(), "the owner still has to hand over first");
    assert!(b.c.transfer_ownership(&cid, &c.acct()).is_err(), "only the owner can");
    let (alice, bob, carol) = (a.acct(), b.acct(), c.acct());
    a.c.transfer_ownership(&cid, &bob).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    for p in [&a, &b, &c] {
        let co = &p.c.st.communities[&cid];
        assert_eq!(co.role_of(&bob), Some(Role::Owner), "{}", p.name);
        assert_eq!(co.role_of(&alice), Some(Role::Admin), "{}", p.name);
        assert_eq!(co.role_of(&carol), Some(Role::Admin), "{}", p.name);
    }
    for g in [&cid, &gen] { assert_eq!(a.c.epoch_fingerprint(g), b.c.epoch_fingerprint(g)); assert_eq!(a.c.epoch_fingerprint(g), c.c.epoch_fingerprint(g)); }
    let (api, k) = b.c.net_handle().unwrap();
    assert_eq!(api.acl_get(&k, &cid).unwrap().owner, bob, "the relay follows");
    // the new owner has full powers, in rooms too; an admin's grant, re-signed, still works at the relay
    assert!(a.c.transfer_ownership(&cid, &carol).is_err(), "the old owner has no say any more");
    let link = c.c.create_invite(&cid, Some(1), 3600).unwrap();
    d.c.join_with_link(&link).unwrap();
    settle(&mut [&mut a, &mut b, &mut c, &mut d]);
    assert!(d.c.channels_of(&cid).iter().all(|x| x.joined), "{:?}", notices(&c));
    b.c.kick(&cid, &carol).unwrap();
    settle(&mut [&mut a, &mut b, &mut c, &mut d]);
    assert!(!b.c.group_accounts(&gen).contains(&carol));
    // and the old owner can now leave like anyone
    a.c.leave(&cid).unwrap();
    settle(&mut [&mut a, &mut b, &mut d]);
    assert!(a.c.st.communities[&cid].removed, "{:?}", notices(&b));
    b.c.send_text(&gen, "under new management", None).unwrap();
    settle(&mut [&mut b, &mut d]);
    assert!(has_text(&d, &gen, "under new management"));
}

#[test]
fn a_demoted_admin_loses_room_powers_too() {
    let net = Net::new();
    let (mut a, mut b, mut c) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"), Peer::new(&net, "carol"));
    for p in [&mut a, &mut b, &mut c] { p.tick(); }
    let (cid, gen) = community_with(&net, &mut a, &mut [&mut b, &mut c]);
    a.c.set_role(&cid, &b.acct(), Role::Admin).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    a.c.set_role(&cid, &b.acct(), Role::Member).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    let t = c.acct();
    let _ = b.c.debug_commit_remove(&gen, &t);
    settle(&mut [&mut a, &mut b, &mut c]);
    assert!(a.c.group_accounts(&gen).contains(&t), "a room's own table no longer lists him");
    assert_eq!(a.c.epoch_fingerprint(&gen), c.c.epoch_fingerprint(&gen));
}

#[test]
fn the_attachment_cache_is_tied_to_the_key() {
    let net = Net::new();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (_, gen) = owner_and(&net, &mut a, &mut b);
    let id = a.c.send_file(&gen, "real.txt", "text/plain", b"secret bytes", "").unwrap();
    settle(&mut [&mut a, &mut b]);
    assert_eq!(b.c.fetch_file(&gen, &id).unwrap().2, b"secret bytes");
    // somebody names the same blob, with their own key, file name and type
    let mut fake = b.c.st.chans[&gen].msgs.iter().find(|m| m.id == id).unwrap().clone();
    fake.id = "f".repeat(32);
    if let MsgBody::File { key, name, .. } = &mut fake.body { *key = "00".repeat(32); *name = "evil.html".into(); }
    b.c.st.chans.get_mut(&gen).unwrap().msgs.push(fake);
    assert!(b.c.fetch_file(&gen, &"f".repeat(32)).is_err(), "the cached bytes are not served under another key");
}

#[test]
fn reactions_are_capped() {
    let net = Net::new();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (_, gen) = owner_and(&net, &mut a, &mut b);
    let id = a.c.send_text(&gen, "react to me", None).unwrap();
    settle(&mut [&mut a, &mut b]);
    for e in ["😀", "😁", "😂", "😃", "😄", "😅", "😆", "😇", "😈", "😉", "😊"] { a.c.react(&gen, &id, e, true).unwrap(); }
    settle(&mut [&mut a, &mut b]);
    assert_eq!(b.c.messages(&gen).iter().find(|m| m.id == id).unwrap().reactions.len(), MAX_REACTIONS_PER_ACCOUNT);
    assert!(a.c.react(&gen, &id, "<img src=x>", true).is_err(), "emoji, not markup");
    assert!(a.c.react(&gen, &id, "1\u{fe0f}\u{20e3}", false).is_ok(), "keycaps are emoji");
}

#[test]
fn a_timestamp_cannot_come_from_the_future() {
    let now = commons_proto::now_secs();
    assert!(commons_core::sync::sane_ts(u64::MAX) <= now + 301);
    assert_eq!(commons_core::sync::sane_ts(now - 86400), now - 86400, "an outbox can be days old");
}

// ---- item 16
#[test]
fn people_can_be_told_apart() {
    let net = Net::new();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (cid, gen) = owner_and(&net, &mut a, &mut b);
    assert!(b.c.set_name("ALICE").is_err() && b.c.set_name("al ice").is_err() && b.c.set_name("ali\u{200b}ce").is_err(), "taken, however it is dressed up");
    assert!(a.c.create_channel(&cid, "General").is_err(), "one room per name");
    assert!(a.members_plain(&cid).iter().all(|m| !m.same_name));
    // a modified client ignores that: everyone can still see who is who
    b.c.st.name = "alice".into(); b.c.st.profile_dirty = true;
    settle(&mut [&mut a, &mut b]);
    let ms = a.members_plain(&cid);
    assert!(ms.iter().all(|m| m.same_name), "{ms:?}");
    assert_ne!(ms[0].tag, ms[1].tag);
    assert_eq!(a.c.people_of(&cid)[&a.acct()].0, Role::Owner);
    let _ = gen;
}
impl Peer { fn members_plain(&self, cid: &str) -> Vec<MemberInfo> { self.c.members(cid) } }

// ---- low: recovery keys
#[test]
fn recovery_keys_must_be_exact() {
    let k = encode_recovery(&[7u8; 32]);
    assert_eq!(k.chars().filter(|c| *c != '-').count(), 56);
    assert_eq!(decode_recovery(&k).unwrap(), [7u8; 32]);
    assert_eq!(decode_recovery(&k.to_lowercase().replace('-', " ")).unwrap(), [7u8; 32]);
    assert!(decode_recovery(&format!("{k}JUNK")).is_err(), "trailing characters are a typo");
    assert!(decode_recovery(&k[..k.len() - 2]).is_err());
}

// ---- low: quotas and the registration gate
#[test]
fn the_relay_gate_and_quotas_hold() {
    use commons_core::transport::*;
    use commons_proto::*;
    use std::time::Duration;
    let lim = commons_relay::Limits { invite_secret: Some("letmein".into()), max_blob_bytes_per_account: 2000, max_entry_bytes_per_account: 3000, max_inbox_items: 2, kp_takes_per_target_per_window: 2, max_devices_per_account: 2, ..Default::default() };
    let net = Net::with_limits(lim);
    let dir = tempfile::tempdir().unwrap();
    let d = Duration::from_secs(5);
    let c = Client::create(dir.path(), "pw", &net.url(), "nobody", KdfParams::TEST, transport(&net.url()).0).unwrap();
    assert!(!c.st.registered, "no secret, no account");
    drop(c);
    let dir = tempfile::tempdir().unwrap();
    let mut c = Client::create_with_invite(dir.path(), "pw", &net.url(), "friend", "letmein", KdfParams::TEST, transport(&net.url()).0).unwrap();
    assert!(c.st.registered);
    let api = Api { t: transport(&net.url()).0 };
    assert!(api.info(None).unwrap().gated);
    assert!(api.info(Some("letmein")).unwrap().ok && !api.info(Some("nope")).unwrap().ok);
    // another device of an account that is already in needs no secret, up to the device limit
    let key = c.recovery_key().unwrap();
    let dir2 = tempfile::tempdir().unwrap();
    assert!(Client::create_from_recovery(dir2.path(), "pw", &net.url(), "friend", &key, KdfParams::TEST, transport(&net.url()).0).unwrap().st.registered);
    let dir3 = tempfile::tempdir().unwrap();
    assert!(!Client::create_from_recovery(dir3.path(), "pw", &net.url(), "friend", &key, KdfParams::TEST, transport(&net.url()).0).unwrap().st.registered, "too many devices");
    let (_, k) = c.net_handle().unwrap();
    let t = HttpTransport::new(&net.url());
    let id = || hex::encode(rand::random::<[u8; 16]>());
    assert_eq!(t.request(Some(&k), "PUT", &format!("/v1/blobs/{}", id()), &[0u8; 1500], d).unwrap().status, 200);
    assert_eq!(t.request(Some(&k), "PUT", &format!("/v1/blobs/{}", id()), &[0u8; 1500], d).unwrap().status, 507, "file quota");
    let gid = id();
    api.acl(&k, &gid, &AclReq::default()).unwrap();
    let post = || { let b = serde_json::to_vec(&PostReq { id: id(), data: b64(&[1u8; 1400]) }).unwrap(); t.request(Some(&k), "POST", &format!("/v1/groups/{gid}/messages"), &b, d).unwrap().status };
    assert_eq!((post(), post(), post()), (200, 200, 507), "log quota");
    let dev = c.my_device();
    let welcome = || { let b = serde_json::to_vec(&PostReq { id: id(), data: b64(&[1u8; 10]) }).unwrap(); t.request(Some(&k), "POST", &format!("/v1/welcomes/{dev}"), &b, d).unwrap().status };
    assert_eq!((welcome(), welcome(), welcome()), (200, 200, 429), "inbox limit");
    let nobody = "0".repeat(64);
    let b = serde_json::to_vec(&PostReq { id: id(), data: b64(&[1u8; 10]) }).unwrap();
    assert_eq!(t.request(Some(&k), "POST", &format!("/v1/welcomes/{nobody}"), &b, d).unwrap().status, 404);
    let take = || t.request(Some(&k), "GET", &format!("/v1/keypackages/{dev}"), &[], d).unwrap().status;
    assert_eq!((take(), take(), take()), (200, 200, 429), "take limit");
    let _ = c.tick(0);
}

#[test]
fn two_admins_racing_leave_no_scar_in_the_room() {
    let net = Net::new();
    let (mut a, mut b, mut c) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"), Peer::new(&net, "carol"));
    for p in [&mut a, &mut b, &mut c] { p.tick(); }
    let (cid, gen) = community_with(&net, &mut a, &mut [&mut b, &mut c]);
    a.c.set_role(&cid, &b.acct(), Role::Admin).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    let t = c.acct();
    a.c.kick(&cid, &t).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    for p in [&a, &b] { assert!(p.c.messages(&gen).iter().all(|m| !matches!(m.body, MsgBody::Notice(_))), "{}: {:?}", p.name, p.c.messages(&gen)); }
}

#[test]
fn a_group_the_relay_lost_is_not_quietly_started_again() {
    let mut net = Net::new();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (cid, gen) = owner_and(&net, &mut a, &mut b);
    a.c.send_text(&gen, "before the wipe", None).unwrap();
    settle(&mut [&mut a, &mut b]);
    net.stop_relay();
    std::fs::remove_dir_all(net.data.path().join("relay")).unwrap();
    net.start_relay();
    for _ in 0..4 { a.tick(); b.tick(); }
    assert!(a.c.st.groups[&cid].denied && a.c.st.groups[&gen].denied, "it stays visibly broken");
    assert!(notices(&a).iter().any(|n| n.starts_with("relay:")));
    assert_eq!(a.c.group_status(&gen), "the relay is not letting this account in");
}

// ---- low: a recorded rejoin request cannot be replayed to churn a device's leaf
#[test]
fn a_replayed_rejoin_request_changes_nothing() {
    use ed25519_dalek::Signer;
    let net = Net::new();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (_cid, gen) = owner_and(&net, &mut a, &mut b);
    settle(&mut [&mut a, &mut b]);
    let (api, k) = b.c.net_handle().unwrap();
    let cert = b.c.st.cert.clone().unwrap();
    let nonce = rand_hex(8);
    let mut m = b"commons/rejoin/v1".to_vec();
    m.extend_from_slice(&commons_proto::sha256(&[gen.as_bytes(), nonce.as_bytes()]));
    let body = serde_json::json!({"cert": cert, "nonce": nonce, "sig": hex::encode(k.sign(&m).to_bytes())});
    let mut data = vec![2u8]; // TAG_REJOIN
    data.extend(serde_json::to_vec(&body).unwrap());

    let before = a.c.epoch_fingerprint(&gen);
    api.post_entry(&k, &gen, &rand_hex(16), &data).unwrap();
    settle(&mut [&mut a, &mut b]);
    let after_first = a.c.epoch_fingerprint(&gen);
    assert_ne!(before, after_first, "the first, genuine request is acted on");
    assert_eq!(after_first, b.c.epoch_fingerprint(&gen));

    // The same bytes again, as a relay or an eavesdropper replaying a recorded request could send them.
    api.post_entry(&k, &gen, &rand_hex(16), &data).unwrap();
    settle(&mut [&mut a, &mut b]);
    assert_eq!(a.c.epoch_fingerprint(&gen), after_first, "a replayed request must not rotate the device's leaf again");
    b.c.send_text(&gen, "still fine", None).unwrap();
    settle(&mut [&mut a, &mut b]);
    assert!(has_text(&a, &gen, "still fine"));
}

// ---- low: the recovery key is the whole identity, so showing it needs the passphrase again
#[test]
fn recovery_key_needs_the_passphrase() {
    let net = Net::new();
    let a = Peer::new(&net, "alice"); // passphrase "pw"
    assert!(a.c.recovery_key_with_passphrase("not-the-passphrase").is_err());
    assert!(a.c.recovery_key_with_passphrase("").is_err());
    assert_eq!(a.c.recovery_key_with_passphrase("pw").unwrap(), a.c.recovery_key().unwrap());

    // the same through the door the UI uses
    let dir = tempfile::tempdir().unwrap();
    let app = commons_core::api::App::with_kdf(dir.path().to_path_buf(), &net.url(), KdfParams::TEST);
    app.call("create_account", serde_json::json!({"name": "A", "passphrase": "correct horse battery", "relay": net.url()})).unwrap();
    assert!(app.call("recovery_key", serde_json::json!({"passphrase": "wrong"})).is_err());
    assert!(app.call("recovery_key", serde_json::json!({})).is_err(), "no passphrase at all");
    let shown = app.call("recovery_key", serde_json::json!({"passphrase": "correct horse battery"})).unwrap();
    assert!(shown["key"].as_str().is_some_and(|k| decode_recovery(k).is_ok()));
    app.shutdown();
}
