//! Owner colours (carried in the signed community table) and the live channel (best-effort, never history).
mod common;
use common::*;
use commons_core::envelope::*;
use commons_core::state::Role;
use commons_proto::now_secs;

fn duo(m: &str, h: &str) -> Duo { Duo { main: m.into(), hi: h.into() } }
fn trio() -> (Net, Peer, Peer, Peer, String, String) {
    let net = Net::new();
    let (mut a, mut b, mut c) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"), Peer::new(&net, "carol"));
    let (cid, gen) = community_with(&net, &mut a, &mut [&mut b, &mut c]);
    (net, a, b, c, cid, gen)
}
fn room_colours(p: &Peer, cid: &str, gid: &str) -> Option<Duo> { p.c.st.communities[cid].style.as_ref().and_then(|s| s.rooms.get(gid).cloned()) }

// ------------------------------------------------------------------------------------------------ compatibility
/// What an older Megatron has: no `Live` kind, and a control table with no `style` field.
#[derive(serde::Deserialize, Debug)]
#[allow(dead_code)]
#[serde(tag = "t")]
enum OldEnvelope { Typing, Text { text: String }, #[serde(other)] Unknown }
#[derive(serde::Deserialize, Debug)]
#[allow(dead_code)]
#[serde(tag = "k")]
enum OldExt { Control { name: String, owner: String, channels: Vec<serde_json::Value> }, Channel { name: String } }

#[test]
fn an_old_client_ignores_live_events_and_reads_a_table_with_colours() {
    let live = Envelope::Live { kind: "cursor".into(), data: "{\"x\":1}".into(), ts: now_secs() };
    // Old app: an unknown kind, which it skips with the "newer version" note it already has. It is not read as typing or text.
    assert!(matches!(serde_json::from_slice::<OldEnvelope>(&live.to_bytes()).unwrap(), OldEnvelope::Unknown));
    // New app: round trip.
    assert_eq!(Envelope::parse(&live.to_bytes()).unwrap(), live);

    let key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
    let owner = hex::encode(key.verifying_key().to_bytes());
    let nonce = "a".repeat(32);
    let cid = community_id(&owner, &nonce);
    let doc = StyleDoc { ver: 1, ts: 5, community: Some(duo("#112233", "#445566")), rooms: Default::default() };
    let ext = GroupExt::Control { name: "n".into(), owner: owner.clone(), nonce, admins: vec![], channels: vec![], invites: vec![], banned: vec![], transfers: vec![], consents: vec![], style: Some(sign_style(&key, &cid, &doc).unwrap()) };
    match serde_json::from_slice::<OldExt>(&ext.to_bytes()).unwrap() { OldExt::Control { name, .. } => assert_eq!(name, "n"), _ => panic!() }
    let parsed = GroupExt::parse(&ext.to_bytes()).unwrap();
    assert_eq!(parsed, ext);
    // A table with a damaged style field still reads, as "no colours".
    let mut v: serde_json::Value = serde_json::from_slice(&ext.to_bytes()).unwrap();
    v["style"] = serde_json::json!({"doc": 5});
    let GroupExt::Control { style, .. } = GroupExt::parse(&serde_json::to_vec(&v).unwrap()).unwrap() else { panic!() };
    assert!(style.is_none());
}

#[test]
fn live_events_are_checked_strictly() {
    let ok = |k: &str, d: &str| Envelope::parse(&Envelope::Live { kind: k.into(), data: d.into(), ts: 1 }.to_bytes()).is_ok();
    assert!(ok("cursor", "x"));
    assert!(ok("a.b_c-1", &"x".repeat(MAX_LIVE)));
    assert!(!ok("cursor", &"x".repeat(MAX_LIVE + 1)));
    assert!(!ok("", "x"));
    assert!(!ok("Cursor", "x"));
    assert!(!ok("a b", "x"));
    assert!(!ok(&"k".repeat(33), "x"));
}

#[test]
fn colours_are_validated_strictly_and_only_the_owner_signature_counts() {
    for good in ["#000000", "#abcdef", "#ABCDEF", "#0a1B2c"] { assert!(valid_color(good), "{good}"); }
    for bad in ["", "#fff", "000000", "#12345g", "#1234567", "red", "#12 456", "#12345\n", "javascript:", "#ffffff;x", "rgb(0,0,0)", "#ＡＡＡＡＡＡ"] { assert!(!valid_color(bad), "{bad:?}"); }
    let owner = ed25519_dalek::SigningKey::from_bytes(&[1u8; 32]);
    let other = ed25519_dalek::SigningKey::from_bytes(&[2u8; 32]);
    let ohex = hex::encode(owner.verifying_key().to_bytes());
    let doc = StyleDoc { ver: 1, ts: 1, community: None, rooms: [("abcdef01".to_string(), duo("#101010", "#202020"))].into() };
    let good = sign_style(&owner, "cc", &doc).unwrap();
    assert_eq!(verify_style(&ohex, "cc", &good).unwrap(), doc);
    assert!(verify_style(&ohex, "other-community", &good).is_err(), "signed for one community only");
    assert!(verify_style(&ohex, "cc", &sign_style(&other, "cc", &doc).unwrap()).is_err(), "someone else's signature");
    let mut tampered = good.clone(); tampered.doc = tampered.doc.replace("101010", "111111");
    assert!(verify_style(&ohex, "cc", &tampered).is_err());
    // A bad colour cannot be signed in the first place, and a signed one with a bad colour is refused.
    let mut bad = doc.clone(); bad.rooms.insert("abcdef01".into(), duo("#101010", "red"));
    assert!(sign_style(&owner, "cc", &bad).is_err());
    let mut zero = doc.clone(); zero.ver = 0;
    assert!(sign_style(&owner, "cc", &zero).is_err());
}

// ------------------------------------------------------------------------------------------------ colours end to end
#[test]
fn the_owner_sets_colours_and_everyone_including_a_late_joiner_gets_them() {
    let (net, mut a, mut b, mut c, cid, gen) = trio();
    a.c.set_style(&cid, Some(&gen), Some(duo("#AA3300", "#ffcc00"))).unwrap();
    a.c.set_style(&cid, None, Some(duo("#003366", "#66ccff"))).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    for p in [&a, &b, &c] {
        assert_eq!(room_colours(p, &cid, &gen), Some(duo("#aa3300", "#ffcc00")), "{} sees the room colours (lower-cased)", p.name);
        assert_eq!(p.c.st.communities[&cid].style.as_ref().unwrap().community, Some(duo("#003366", "#66ccff")));
        assert_eq!(p.c.st.communities[&cid].style.as_ref().unwrap().ver, 2);
    }
    // Someone who joins afterwards reads them out of the table in the Welcome.
    let mut d = Peer::new(&net, "dave");
    let link = a.c.create_invite(&cid, None, 3600).unwrap();
    d.c.join_with_link(&link).unwrap();
    settle(&mut [&mut a, &mut b, &mut c, &mut d]);
    assert_eq!(room_colours(&d, &cid, &gen), Some(duo("#aa3300", "#ffcc00")), "a late joiner gets the colours");
    // And so does the same device after a restart.
    d.reopen(&net);
    assert_eq!(room_colours(&d, &cid, &gen), Some(duo("#aa3300", "#ffcc00")));
    // Resetting one room puts the default back (no entry), the other stays.
    a.c.set_style(&cid, Some(&gen), None).unwrap();
    settle(&mut [&mut a, &mut b, &mut c, &mut d]);
    assert_eq!(room_colours(&b, &cid, &gen), None);
    assert!(b.c.st.communities[&cid].style.as_ref().unwrap().community.is_some());
}

#[test]
fn only_the_owner_can_set_colours_even_an_admin_and_forgeries_are_ignored() {
    let (_net, mut a, mut b, mut c, cid, gen) = trio();
    a.c.set_role(&cid, &b.acct(), Role::Admin).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    a.c.set_style(&cid, Some(&gen), Some(duo("#111111", "#222222"))).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    assert!(b.c.set_style(&cid, Some(&gen), Some(duo("#999999", "#888888"))).is_err(), "an admin is refused");
    assert!(c.c.set_style(&cid, Some(&gen), Some(duo("#999999", "#888888"))).is_err(), "a member is refused");
    assert!(a.c.set_style(&cid, Some(&gen), Some(duo("red", "#888888"))).is_err(), "bad colours are refused");
    assert!(a.c.set_style(&cid, Some(&gen), Some(duo("#12345", "#888888"))).is_err());
    assert!(a.c.set_style(&cid, Some("deadbeefdeadbeef"), Some(duo("#999999", "#888888"))).is_err(), "not a room of this community");

    // The admin writes colours into the table under his own key: a forgery, ignored by everyone.
    let bkey = b.c.debug_account_key();
    let forged = sign_style(&bkey, &cid, &StyleDoc { ver: 99, ts: now_secs(), community: None, rooms: [(gen.clone(), duo("#ff0000", "#00ff00"))].into() }).unwrap();
    b.c.debug_plant_style(&cid, Some(forged)).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    for p in [&a, &b, &c] { assert_eq!(room_colours(p, &cid, &gen), Some(duo("#111111", "#222222")), "{} ignored the forgery", p.name); }

    // He cannot replay an older document of the owner's either: its version is lower than the one in force.
    let old = a.c.st.communities[&cid].style.clone().unwrap();
    a.c.set_style(&cid, Some(&gen), Some(duo("#333333", "#444444"))).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    let replay = sign_style(&a.c.debug_account_key(), &cid, &old).unwrap();
    b.c.debug_plant_style(&cid, Some(replay)).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    for p in [&a, &b, &c] { assert_eq!(room_colours(p, &cid, &gen), Some(duo("#333333", "#444444")), "{} kept the newer colours", p.name); }
}

#[test]
fn colours_come_back_after_a_table_rewrite_that_dropped_them() {
    let (_net, mut a, mut b, mut c, cid, gen) = trio();
    a.c.set_role(&cid, &b.acct(), Role::Admin).unwrap();
    a.c.set_style(&cid, Some(&gen), Some(duo("#111111", "#222222"))).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    // What an older admin's app does when it renames or adds a room: rewrites the table without the field it does not know.
    b.c.debug_plant_style(&cid, None).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    // Nobody lost them on screen, and the owner's device put them back in the table.
    for p in [&a, &b, &c] { assert_eq!(room_colours(p, &cid, &gen), Some(duo("#111111", "#222222")), "{}", p.name); }
    assert!(a.c.st.communities[&cid].style.as_ref().unwrap().ver >= 2, "re-signed with a higher version");
}

// ------------------------------------------------------------------------------------------------ live channel
fn live_kinds(p: &Peer, since: u64) -> Vec<(String, String, String)> { p.c.live_poll(since, None).into_iter().map(|e| (e.account, e.kind, e.data)).collect() }

#[test]
fn a_live_event_reaches_a_friend_and_is_never_history() {
    let net = Net::new();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (_cid, gen) = community_with(&net, &mut a, &mut [&mut b]);
    settle(&mut [&mut a, &mut b]);
    let msgs_before = (a.c.messages(&gen).len(), b.c.messages(&gen).len());
    let unread_before = b.c.unread(&gen);
    assert!(a.c.live_send(&gen, "cursor", "{\"x\":3,\"y\":4}").unwrap());
    settle(&mut [&mut a, &mut b]);
    let evs = b.c.live_poll(0, None);
    assert_eq!(evs.len(), 1);
    assert_eq!((evs[0].account.as_str(), evs[0].kind.as_str(), evs[0].data.as_str(), evs[0].gid.as_str()), (a.acct().as_str(), "cursor", "{\"x\":3,\"y\":4}", gen.as_str()));
    assert!(evs[0].id >= 1);
    assert!(b.c.live_poll(evs[0].id, None).is_empty(), "reading from the newest id gives nothing");
    assert!(a.c.live_poll(0, None).is_empty(), "no echo to the sender");
    // Not history, not unread, no notice, and it is gone from the sender's queue.
    assert_eq!((a.c.messages(&gen).len(), b.c.messages(&gen).len()), msgs_before);
    assert_eq!(b.c.unread(&gen), unread_before);
    assert!(!notices(&b).iter().any(|n| n.contains("newer")), "{:?}", notices(&b));
    assert_eq!(a.c.outbox_len(), 0);
    // It is encrypted like a message: nothing readable on the relay.
    assert!(!contains(&relay_bytes(&net), "cursor"));
    // A restart does not bring it back.
    b.reopen(&net);
    assert!(b.c.live_poll(0, None).is_empty());
}

#[test]
fn live_sending_is_limited_and_coalesced_and_receiving_is_capped() {
    let net = Net::new();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (_cid, gen) = community_with(&net, &mut a, &mut [&mut b]);
    settle(&mut [&mut a, &mut b]);
    // Twenty updates of one kind in a burst: the first goes, and while the rest wait only the newest is kept.
    for i in 0..20 { a.c.live_send(&gen, "cursor", &format!("{i}")).unwrap(); }
    assert!(a.c.outbox_len() <= 4, "at most four a second per room, got {}", a.c.outbox_len());
    settle(&mut [&mut a, &mut b]);
    let got = live_kinds(&b, 0);
    assert!(got.len() <= 4 && !got.is_empty(), "{got:?}");
    assert_eq!(got[0].2, "0");
    // Four different kinds go at once; a fifth waits.
    for k in ["a", "b", "c", "d", "e"] { a.c.live_send(&gen, k, "x").unwrap(); }
    assert!(a.c.outbox_len() <= 4, "outbox {}", a.c.outbox_len());
    // Bad input is refused, not clipped.
    assert!(a.c.live_send(&gen, "Bad Kind", "x").is_err());
    assert!(a.c.live_send(&gen, "k", &"x".repeat(MAX_LIVE + 1)).is_err());
    assert!(a.c.live_send("not-a-room", "k", "x").is_err());

    // A sender that ignores its own limit (a hostile client) is held to a per-second share and the buffer is capped.
    settle(&mut [&mut a, &mut b]);
    for i in 0..300 { a.c.debug_send_envelope(&gen, Envelope::Live { kind: "spam".into(), data: format!("{i}"), ts: now_secs() }).unwrap(); }
    settle(&mut [&mut a, &mut b]);
    assert!(b.c.live_poll(0, None).len() <= commons_core::client::LIVE_BUFFER);
    let spam = b.c.live_poll(0, None).into_iter().filter(|e| e.kind == "spam").count();
    assert!(spam < 120, "one sender gets a share per second, not all 300 ({spam})");
}

#[test]
fn stale_oversized_and_wrong_room_live_events_are_dropped() {
    let net = Net::new();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (cid, gen) = community_with(&net, &mut a, &mut [&mut b]);
    settle(&mut [&mut a, &mut b]);
    let now = now_secs();
    a.c.debug_send_envelope(&gen, Envelope::Live { kind: "old".into(), data: "x".into(), ts: now - 3600 }).unwrap();
    a.c.debug_send_envelope(&gen, Envelope::Live { kind: "future".into(), data: "x".into(), ts: now + 3600 }).unwrap();
    a.c.debug_send_envelope(&gen, Envelope::Live { kind: "big".into(), data: "x".repeat(MAX_LIVE + 1), ts: now }).unwrap();
    a.c.debug_send_envelope(&gen, Envelope::Live { kind: "BAD".into(), data: "x".into(), ts: now }).unwrap();
    // On the community table (not a room) it is the wrong kind of group and is ignored.
    a.c.debug_send_envelope(&cid, Envelope::Live { kind: "table".into(), data: "x".into(), ts: now }).unwrap();
    a.c.debug_send_envelope(&gen, Envelope::Live { kind: "fine".into(), data: "x".into(), ts: now }).unwrap();
    settle(&mut [&mut a, &mut b]);
    let kinds: Vec<String> = b.c.live_poll(0, None).into_iter().map(|e| e.kind).collect();
    assert_eq!(kinds, vec!["fine".to_string()]);
}

#[test]
fn a_removed_members_live_events_are_rejected() {
    let net = Net::new();
    let (mut a, mut b, mut c) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"), Peer::new(&net, "carol"));
    let (cid, gen) = community_with(&net, &mut a, &mut [&mut b, &mut c]);
    settle(&mut [&mut a, &mut b, &mut c]);
    // Works while a member.
    b.c.live_send(&gen, "cursor", "1").unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    assert_eq!(live_kinds(&a, 0).len(), 1);
    let seen = a.c.live_poll(0, None).last().unwrap().id;
    let seen_c = c.c.live_poll(0, None).last().unwrap().id;

    a.c.kick(&cid, &b.acct()).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    // He is out: his own client no longer sends, and a forced attempt reaches no one.
    let _ = b.c.live_send(&gen, "cursor", "2");
    let _ = b.c.debug_send_envelope(&gen, Envelope::Live { kind: "cursor".into(), data: "after".into(), ts: now_secs() });
    settle(&mut [&mut a, &mut b, &mut c]);
    assert!(a.c.live_poll(seen, None).is_empty(), "alice took nothing from the removed member: {:?}", live_kinds(&a, seen));
    assert!(c.c.live_poll(seen_c, None).is_empty());
    // Even the ones already in the buffer are not handed out for a room we cannot read: carol still can.
    a.c.live_send(&gen, "cursor", "3").unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    assert!(c.c.live_poll(seen_c, None).iter().any(|e| e.data == "3"));
    assert!(b.c.live_poll(0, None).iter().all(|e| e.data != "3"), "the removed member gets nothing new");
}

#[test]
fn new_accounts_have_typing_hints_on_and_a_stored_choice_is_kept() {
    let net = Net::new();
    let mut a = Peer::new(&net, "alice");
    assert!(a.c.st.typing_enabled, "on for a new account");
    a.c.st.typing_enabled = false;
    a.c.save().unwrap();
    a.reopen(&net);
    assert!(!a.c.st.typing_enabled, "an account that turned it off keeps it off");
}
