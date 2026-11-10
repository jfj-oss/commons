//! Owner look settings (card pattern, chat pattern, backdrop strength) and the community card's own wall: validation, signing, forgeries,
//! compatibility with 0.3.0 both ways, and the guarantee that `cwall.*` never touches a room's wall.
mod common;
use common::*;
use commons_core::envelope::*;
use commons_core::state::Role;
use commons_core::wall::*;
use commons_proto::now_secs;

fn trio() -> (Net, Peer, Peer, Peer, String, String) {
    let net = Net::new();
    let (mut a, mut b, mut c) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"), Peer::new(&net, "carol"));
    let (cid, gen) = community_with(&net, &mut a, &mut [&mut b, &mut c]);
    (net, a, b, c, cid, gen)
}
fn look(card: Option<u8>, chat: Option<u8>, dim: Option<u16>) -> Look { Look { card, chat, dim, oracle_quiet: None, oracle_draw: None } }
fn looks_of(p: &Peer, cid: &str) -> std::collections::BTreeMap<String, Look> { p.c.st.communities[cid].style.as_ref().map(|s| s.looks.clone()).unwrap_or_default() }
fn cglyphs(p: &Peer, cid: &str) -> Vec<(u16, u8)> { p.c.wall_view(&format!("{CWALL_PREFIX}{cid}")).cells.into_iter().filter(|c| c.1 != 0).map(|c| (c.0, c.1)).collect() }
fn rglyphs(p: &Peer, gid: &str) -> Vec<(u16, u8)> { p.c.wall_view(gid).cells.into_iter().filter(|c| c.1 != 0).map(|c| (c.0, c.1)).collect() }

/// The style document exactly as 0.3.0 defines it (no `looks`), and the three steps its `verify_style` takes.
#[derive(serde::Deserialize, serde::Serialize, Debug, PartialEq, Eq, Default)]
struct V030Doc {
    ver: u64, ts: u64,
    #[serde(default)] community: Option<Duo>,
    #[serde(default)] rooms: std::collections::BTreeMap<String, Duo>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")] locked: std::collections::BTreeSet<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")] wall: std::collections::BTreeMap<String, u64>,
}
fn v030_verify(owner: &str, cid: &str, s: &SignedStyle) -> Result<V030Doc, String> {
    use ed25519_dalek::Verifier;
    let vk = ed25519_dalek::VerifyingKey::from_bytes(&commons_proto::unhex32(owner).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let sig = ed25519_dalek::Signature::from_slice(&hex::decode(&s.sig).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    vk.verify(&style_msg(cid, &s.doc), &sig).map_err(|_| "not signed by the owner".to_string())?;
    let d: V030Doc = serde_json::from_str(&s.doc).map_err(|e| e.to_string())?;
    if d.ver == 0 || d.ver > (1 << 53) || d.rooms.len() > MAX_STYLE_ROOMS { return Err("bad".into()); }
    Ok(d)
}

// ------------------------------------------------------------------------------------------------ the document
#[test]
fn looks_are_validated_signed_and_ignored_by_a_030_reader() {
    let key = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]);
    let owner = hex::encode(key.verifying_key().to_bytes());
    let room = "abcdef0123456789".to_string();
    let cid = "cc0011cc0011cc00";
    // Without looks the text is exactly what it always was.
    let plain = StyleDoc { ver: 3, ts: 7, ..Default::default() };
    assert_eq!(sign_style(&key, cid, &plain).unwrap().doc, "{\"ver\":3,\"ts\":7,\"community\":null,\"rooms\":{}}");
    // With looks: signs, verifies, round-trips, and only the parts that are set are printed.
    let full = StyleDoc { ver: 4, ts: 9, rooms: [(room.clone(), Duo { main: "#112233".into(), hi: "#445566".into() })].into(), locked: [cid.to_string()].into(), wall: [(cid.to_string(), 1)].into(),
        looks: [(room.clone(), look(Some(3), None, Some(0))), (cid.to_string(), look(Some(31), None, None))].into(), ..Default::default() };
    let s = sign_style(&key, cid, &full).unwrap();
    assert!(s.doc.contains("\"looks\":{") && s.doc.contains("{\"card\":31}"), "{}", s.doc);
    assert_eq!(verify_style(&owner, cid, &s).unwrap(), full);
    // 0.3.0 verifies the very same signed text, ignores the looks, and still gets the colours, the lock and the clear (community id keys included).
    let old = v030_verify(&owner, cid, &s).unwrap();
    assert_eq!((old.ver, old.rooms[&room].main.as_str(), old.locked.contains(cid), old.wall[cid]), (4, "#112233", true, 1));
    // A forged document (another key) is not believed, and neither is a tampered one.
    let other = ed25519_dalek::SigningKey::from_bytes(&[8u8; 32]);
    assert!(verify_style(&owner, cid, &sign_style(&other, cid, &full).unwrap()).is_err());
    let mut tampered = s.clone(); tampered.doc = tampered.doc.replace("\"card\":3,", "\"card\":4,");
    assert_ne!(tampered.doc, s.doc);
    assert!(verify_style(&owner, cid, &tampered).is_err());
    // What 0.3.0 writes (no looks) is read by this version with empty looks.
    let theirs = V030Doc { ver: 5, ts: 1, rooms: [(room.clone(), Duo { main: "#010203".into(), hi: "#040506".into() })].into(), ..Default::default() };
    let text = serde_json::to_string(&theirs).unwrap();
    let sig = { use ed25519_dalek::Signer; hex::encode(key.sign(&style_msg(cid, &text)).to_bytes()) };
    let read = verify_style(&owner, cid, &SignedStyle { doc: text, sig }).unwrap();
    assert!(read.looks.is_empty() && read.rooms[&room].hi == "#040506");
    // Bounds: patterns 0..=31, strength 0..=200, ids valid, nothing empty, at most MAX_STYLE_ROOMS.
    let bad = |l: Look| StyleDoc { ver: 1, looks: [(room.clone(), l)].into(), ..Default::default() };
    for l in [look(Some(32), None, None), look(None, Some(32), None), look(None, None, Some(201)), look(Some(255), None, None), look(None, None, None)] {
        assert!(sign_style(&key, cid, &bad(l.clone())).is_err(), "{l:?}");
    }
    for l in [look(Some(0), None, None), look(Some(31), Some(31), Some(200)), look(None, None, Some(0))] { assert!(sign_style(&key, cid, &bad(l.clone())).is_ok(), "{l:?}"); }
    assert!(sign_style(&key, cid, &StyleDoc { ver: 1, looks: [("zz".to_string(), look(Some(1), None, None))].into(), ..Default::default() }).is_err());
    let many: std::collections::BTreeMap<String, Look> = (0..=MAX_STYLE_ROOMS).map(|i| (format!("{:016x}", i), look(Some(1), None, None))).collect();
    assert!(sign_style(&key, cid, &StyleDoc { ver: 1, looks: many.clone(), ..Default::default() }).is_err(), "501 entries");
    let mut ok = many; ok.remove(&format!("{:016x}", 0)); assert_eq!(ok.len(), MAX_STYLE_ROOMS);
    assert!(sign_style(&key, cid, &StyleDoc { ver: 1, looks: ok, ..Default::default() }).is_ok());
    // A received document with an out-of-range or unknown-typed look is refused as a whole, even when signed by the owner.
    for text in ["{\"ver\":1,\"ts\":1,\"community\":null,\"rooms\":{},\"looks\":{\"abcdef0123456789\":{\"card\":99}}}", "{\"ver\":1,\"ts\":1,\"community\":null,\"rooms\":{},\"looks\":{\"abcdef0123456789\":{\"dim\":-1}}}",
                 "{\"ver\":1,\"ts\":1,\"community\":null,\"rooms\":{},\"looks\":{\"abcdef0123456789\":{}}}", "{\"ver\":1,\"ts\":1,\"community\":null,\"rooms\":{},\"looks\":[1]}"] {
        let sig = { use ed25519_dalek::Signer; hex::encode(key.sign(&style_msg(cid, text)).to_bytes()) };
        assert!(verify_style(&owner, cid, &SignedStyle { doc: text.into(), sig }).is_err(), "{text}");
    }
    // A pattern number a later version adds (still inside 0..=31) is carried untouched.
    let later = bad(look(Some(20), Some(21), None));
    assert_eq!(verify_style(&owner, cid, &sign_style(&key, cid, &later).unwrap()).unwrap().looks[&room].card, Some(20));
}

// ------------------------------------------------------------------------------------------------ over the wire
#[test]
fn only_the_owner_sets_looks_everyone_gets_them_and_other_edits_keep_them() {
    let (net, mut a, mut b, mut c, cid, gen) = trio();
    a.c.set_role(&cid, &b.acct(), Role::Admin).unwrap();
    let voice = a.c.create_channel_kind(&cid, "talk", "voice").unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    a.c.set_look(&cid, Some(&gen), Some(look(Some(2), Some(4), Some(60)))).unwrap();
    a.c.set_look(&cid, None, Some(look(Some(5), None, None))).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    for p in [&a, &b, &c] {
        let l = looks_of(p, &cid);
        assert_eq!(l.get(&gen), Some(&look(Some(2), Some(4), Some(60))), "{}", p.name);
        assert_eq!(l.get(&cid), Some(&look(Some(5), None, None)), "{}", p.name);
    }
    // Refused at the source: not the owner (an admin included), out of range, an unknown room, chat parts on the community card.
    assert!(b.c.set_look(&cid, Some(&gen), Some(look(Some(1), None, None))).is_err() && c.c.set_look(&cid, None, Some(look(Some(1), None, None))).is_err());
    assert!(a.c.set_look(&cid, Some(&gen), Some(look(Some(32), None, None))).is_err());
    assert!(a.c.set_look(&cid, Some(&gen), Some(look(None, None, Some(201)))).is_err());
    assert!(a.c.set_look(&cid, Some("deadbeefdeadbeef"), Some(look(Some(1), None, None))).is_err());
    assert!(a.c.set_look(&cid, None, Some(look(None, Some(1), None))).is_err() && a.c.set_look(&cid, None, Some(look(None, None, Some(50)))).is_err());
    // An unchanged look publishes nothing (every publish is a permanent log entry).
    let ver = a.c.st.communities[&cid].style.as_ref().unwrap().ver;
    a.c.set_look(&cid, Some(&gen), Some(look(Some(2), Some(4), Some(60)))).unwrap();
    assert_eq!(a.c.st.communities[&cid].style.as_ref().unwrap().ver, ver);
    // Colours, lock and clear keep the looks; a look keeps the colours; the community id is a valid wall key.
    a.c.set_style(&cid, Some(&gen), Some(Duo { main: "#111111".into(), hi: "#222222".into() })).unwrap();
    a.c.set_wall(&cid, &gen, Some(true), true).unwrap();
    a.c.set_wall(&cid, &cid, Some(true), true).unwrap();
    assert!(a.c.set_wall(&cid, "deadbeefdeadbeef", Some(true), false).is_err());
    settle(&mut [&mut a, &mut b, &mut c]);
    for p in [&a, &b, &c] {
        let d = p.c.st.communities[&cid].style.clone().unwrap();
        assert_eq!(d.looks.len(), 2, "{}", p.name);
        assert!(d.locked.contains(&cid) && d.locked.contains(&gen) && d.wall[&cid] == 1 && d.wall[&gen] == 1 && d.rooms.contains_key(&gen), "{}", p.name);
    }
    // Reset: an empty look removes the entry. A voice room can have a look too.
    a.c.set_look(&cid, Some(&gen), None).unwrap();
    a.c.set_look(&cid, Some(&voice), Some(look(Some(1), None, None))).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    for p in [&a, &b, &c] { let l = looks_of(p, &cid); assert!(!l.contains_key(&gen) && l.contains_key(&voice) && l.contains_key(&cid), "{}", p.name); }
    // They survive a restart.
    c.reopen(&net);
    assert!(looks_of(&c, &cid).contains_key(&voice));

    // The admin writes looks into the table under his own key: a forgery nobody believes. A replay of an older owner document does not win either.
    let bkey = b.c.debug_account_key();
    let forged = sign_style(&bkey, &cid, &StyleDoc { ver: 99, ts: now_secs(), looks: [(gen.clone(), look(Some(7), Some(7), Some(200)))].into(), ..Default::default() }).unwrap();
    b.c.debug_plant_style(&cid, Some(forged)).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    for p in [&a, &b, &c] { assert!(!looks_of(p, &cid).contains_key(&gen), "{} ignored the forged look", p.name); }
    let old = a.c.st.communities[&cid].style.clone().unwrap();
    a.c.set_look(&cid, Some(&gen), Some(look(Some(9), None, None))).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    let replay = sign_style(&a.c.debug_account_key(), &cid, &old).unwrap();
    b.c.debug_plant_style(&cid, Some(replay)).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    for p in [&a, &b, &c] { assert_eq!(looks_of(p, &cid).get(&gen), Some(&look(Some(9), None, None)), "{} kept the newer look", p.name); }
}

#[test]
fn a_table_rewritten_by_an_older_app_gets_its_looks_back() {
    let (_net, mut a, mut b, mut c, cid, gen) = trio();
    a.c.set_role(&cid, &b.acct(), Role::Admin).unwrap();
    a.c.set_look(&cid, Some(&gen), Some(look(Some(2), None, Some(80)))).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    // What an older admin's app does when it renames or adds a room: rewrites the table without the document it does not know.
    b.c.debug_plant_style(&cid, None).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    for p in [&a, &b, &c] { assert_eq!(looks_of(p, &cid).get(&gen), Some(&look(Some(2), None, Some(80))), "{}", p.name); }
}

#[test]
fn the_snapshot_lists_only_this_communitys_looks_and_walls() {
    let (_net, mut a, _b, _c, cid, gen) = trio();
    a.c.set_look(&cid, Some(&gen), Some(look(Some(2), Some(3), Some(40)))).unwrap();
    a.c.set_look(&cid, None, Some(look(Some(4), None, None))).unwrap();
    a.c.set_wall(&cid, &cid, Some(true), true).unwrap();
    let snap = commons_core::api::debug_snapshot(&a.c);
    let co = snap["communities"].as_array().unwrap().iter().find(|x| x["id"] == cid.as_str()).unwrap();
    assert_eq!(co["style"]["looks"][&gen], serde_json::json!({"card": 2, "chat": 3, "dim": 40}));
    assert_eq!(co["style"]["looks"][&cid], serde_json::json!({"card": 4, "chat": null, "dim": null}));
    assert_eq!(co["style"]["locked"], serde_json::json!([cid]));
    assert_eq!(co["style"]["wall"][&cid], 1);
}

// ------------------------------------------------------------------------------------------------ the community card's wall
fn ckey(cid: &str) -> String { format!("{CWALL_PREFIX}{cid}") }

#[test]
fn a_community_wall_reaches_everyone_and_never_a_rooms_wall() {
    let (net, mut a, mut b, mut c, cid, gen) = trio();
    let second = a.c.create_channel(&cid, "second").unwrap();
    let voice = a.c.create_channel_kind(&cid, "talk", "voice").unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    let key = ckey(&cid);
    // The carrier is the lowest-id text room.
    let mut texts = vec![gen.clone(), second.clone()]; texts.sort();
    assert_eq!(a.c.wall_carrier(&cid), Some(texts[0].clone()));
    assert!(a.c.wall_draw(&key, &[(3, 4, 1, 0), (4, 4, 2, 1)]).unwrap());
    b.c.wall_draw(&key, &[(10, 10, 5, 1)]).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    let want = vec![(10 * WALL_COLS as u16 + 10, 5u8), (4 * WALL_COLS as u16 + 3, 1), (4 * WALL_COLS as u16 + 4, 2)];
    let sorted = |mut v: Vec<(u16, u8)>| { v.sort(); v };
    for p in [&a, &b, &c] { assert_eq!(sorted(cglyphs(p, &cid)), sorted(want.clone()), "{}", p.name); }
    // No room's wall changed, none of the events reached the feed or the history.
    for p in [&a, &b, &c] { for g in [&gen, &second, &voice] { assert!(rglyphs(p, g).is_empty(), "{}: room wall {g}", p.name); } }
    for p in [&b, &c] { assert!(p.c.live_poll(0, None).is_empty(), "{}", p.name); }
    assert!(b.c.messages(&gen).iter().all(|m| !matches!(m.body, commons_core::state::MsgBody::Text(_))));
    // A room's own wall works at the same time and does not leak into the community's. A voice room has a wall too.
    a.c.wall_draw(&gen, &[(1, 1, 3, 0)]).unwrap();
    a.c.wall_draw(&voice, &[(2, 2, 3, 0)]).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    assert_eq!(rglyphs(&c, &gen), vec![(WALL_COLS as u16 + 1, 3)]);
    assert_eq!(rglyphs(&c, &voice), vec![(2 * WALL_COLS as u16 + 2, 3)]);
    assert_eq!(cglyphs(&c, &cid).len(), 3);
    // Bad strokes sent by a hostile member straight into the carrier room are dropped whole; a good one lands.
    let me = now_secs();
    let send = |p: &mut Peer, kind: &str, data: String| p.c.debug_send_envelope(&texts[1], Envelope::Live { kind: kind.into(), data, ts: me }).unwrap();
    let before = sorted(cglyphs(&c, &cid));
    send(&mut b, "cwall.d", format!("{{\"g\":0,\"k\":9,\"c\":[[1,2],[{},2]]}}", WALL_CELLS));
    send(&mut b, "cwall.d", "{\"g\":0,\"k\":9,\"c\":[[1,2],[2,99]]}".into());
    send(&mut b, "cwall.d", "{\"g\":0,\"k\":0,\"c\":[[1,2]]}".into());
    send(&mut b, "cwall.d", "{\"g\":4,\"k\":9,\"c\":[[1,2]]}".into());
    settle(&mut [&mut a, &mut b, &mut c]);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    send(&mut b, "cwall.d", "not json".into());
    send(&mut b, "cwall.s", "{\"g\":0,\"i\":0,\"n\":1,\"a\":[\"nope\"],\"c\":[[1,2,3,0]]}".into());
    settle(&mut [&mut a, &mut b, &mut c]);
    assert_eq!(sorted(cglyphs(&c, &cid)), before, "none of the bad ones changed anything");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    send(&mut b, "cwall.d", "{\"g\":0,\"k\":9,\"c\":[[1,2],[2,3]]}".into());
    settle(&mut [&mut a, &mut b, &mut c]);
    assert!(cglyphs(&c, &cid).contains(&(1, 1)) && cglyphs(&c, &cid).contains(&(2, 1)));
    // `cwall.*` sent into a room does not count as a stroke on the room's wall, and `wall.*` does not reach the community's.
    assert_eq!(rglyphs(&c, &texts[1]), if texts[1] == gen { vec![(WALL_COLS as u16 + 1, 3)] } else { vec![] });
    // Not a stroke for a community we are not in, and not for a room we are not in.
    assert!(a.c.wall_draw("cw:deadbeefdeadbeef", &[(1, 1, 1, 0)]).is_err());
    assert!(a.c.wall_draw("nonsense", &[(1, 1, 1, 0)]).is_err());
    assert!(a.c.wall_draw(&key, &[(WALL_COLS, 0, 1, 0)]).is_err() && a.c.wall_draw(&key, &[]).is_err());
    // The wall is saved with the state under its own key and survives a restart.
    a.c.debug_flush_walls(); a.c.save().unwrap();
    assert!(a.c.st.walls.contains_key(&key) && !a.c.st.walls.contains_key(&cid));
    a.reopen(&net);
    assert_eq!(cglyphs(&a, &cid).len(), 5);
}

#[test]
fn lock_and_clear_of_a_community_wall_are_the_owners_alone() {
    let (_net, mut a, mut b, mut c, cid, gen) = trio();
    settle(&mut [&mut a, &mut b, &mut c]);
    let key = ckey(&cid);
    b.c.wall_draw(&key, &[(1, 1, 1, 0)]).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    assert!(b.c.set_wall(&cid, &cid, Some(true), false).is_err() && b.c.set_wall(&cid, &cid, None, true).is_err());
    a.c.set_wall(&cid, &cid, Some(true), false).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    for p in [&a, &b, &c] { assert!(p.c.wall_view(&key).locked, "{}", p.name); assert!(!p.c.wall_view(&gen).locked, "{}: the lock is the community card's only", p.name); }
    assert!(b.c.wall_draw(&key, &[(2, 2, 2, 0)]).is_err(), "bob's own client refuses");
    assert!(b.c.wall_draw(&gen, &[(2, 2, 2, 0)]).is_ok(), "rooms stay open");
    let carrier = a.c.wall_carrier(&cid).unwrap();
    // A forced stroke and a snapshot overwrite are dropped by the others; the owner still draws.
    b.c.debug_send_envelope(&carrier, Envelope::Live { kind: "cwall.d".into(), data: "{\"g\":0,\"k\":50,\"c\":[[100,2]]}".into(), ts: now_secs() }).unwrap();
    let snap = format!("{{\"g\":0,\"i\":0,\"n\":1,\"a\":[\"{}\"],\"c\":[[{},4,99,0],[200,2,99,0]]}}", b.acct(), WALL_COLS + 1);
    b.c.debug_send_envelope(&carrier, Envelope::Live { kind: "cwall.s".into(), data: snap, ts: now_secs() }).unwrap();
    a.c.wall_draw(&key, &[(7, 7, 6, 1)]).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    for p in [&a, &c] {
        let g = cglyphs(p, &cid);
        assert!(!g.iter().any(|x| x.0 == 100) && g.contains(&(WALL_COLS as u16 + 1, 1)) && g.contains(&(7 * WALL_COLS as u16 + 7, 6)), "{}: {g:?}", p.name);
    }
    // Clear: generation 1 for everybody; a late stroke of the old generation is ignored; the room's own wall is untouched.
    a.c.set_wall(&cid, &cid, None, true).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    for p in [&a, &b, &c] { assert!(cglyphs(p, &cid).is_empty() && p.c.wall_view(&key).gen == 1, "{}", p.name); assert_eq!(rglyphs(p, &gen).len(), 1, "{}: the room wall stayed", p.name); }
    std::thread::sleep(std::time::Duration::from_millis(1100));
    b.c.debug_send_envelope(&carrier, Envelope::Live { kind: "cwall.d".into(), data: "{\"g\":0,\"k\":60,\"c\":[[5,2]]}".into(), ts: now_secs() }).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    assert!(cglyphs(&a, &cid).is_empty() && cglyphs(&c, &cid).is_empty());
    // Unlock and the members draw again; lock and generation survive a restart.
    a.c.set_wall(&cid, &cid, Some(false), false).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    b.c.wall_draw(&key, &[(9, 9, 2, 0)]).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    assert_eq!(cglyphs(&c, &cid).len(), 1);
    a.c.set_wall(&cid, &cid, Some(true), false).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    c.reopen(&_net);
    assert!(c.c.wall_view(&key).locked && c.c.wall_view(&key).gen == 1);
}

#[test]
fn a_030_document_with_a_community_wall_lock_still_verifies_and_the_carrier_may_change() {
    let (net, mut a, mut b, mut c, cid, gen) = trio();
    settle(&mut [&mut a, &mut b, &mut c]);
    // The owner's real signed document, with the lock and clear keyed by the community id, as a 0.3.0 client reads it.
    a.c.set_wall(&cid, &cid, Some(true), true).unwrap();
    a.c.set_look(&cid, None, Some(look(Some(3), None, None))).unwrap();
    let signed = sign_style(&a.c.debug_account_key(), &cid, a.c.st.communities[&cid].style.as_ref().unwrap()).unwrap();
    let owner = a.acct();
    let old = v030_verify(&owner, &cid, &signed).unwrap();
    assert!(old.locked.contains(&cid) && old.wall[&cid] == 1);
    // A 0.3.0 client filters those to channel ids, so it neither locks nor clears any room by them: the room walls here are not locked.
    assert!(!a.c.wall_view(&gen).locked);
    // Carrier change: the lowest-id room disappears from the list of rooms we can send in; the next one carries, and nobody is confused.
    a.c.set_wall(&cid, &cid, Some(false), false).unwrap();
    let extra = a.c.create_channel(&cid, "extra").unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    let key = ckey(&cid);
    let first = a.c.wall_carrier(&cid).unwrap();
    a.c.wall_draw(&key, &[(1, 1, 1, 0)]).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    assert_eq!(cglyphs(&c, &cid), vec![(WALL_COLS as u16 + 1, 1)]);
    // Bob's device, after a restart, still has the picture (device copies merge), and later strokes through another room still land.
    b.reopen(&net);
    assert_eq!(cglyphs(&b, &cid).len(), 1);
    let other = [&gen, &extra].into_iter().find(|g| **g != first).unwrap().clone();
    a.c.debug_send_envelope(&other, Envelope::Live { kind: "cwall.d".into(), data: format!("{{\"g\":1,\"k\":900,\"c\":[[{},3]]}}", 2 * WALL_COLS), ts: now_secs() }).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    assert!(cglyphs(&c, &cid).contains(&(2 * WALL_COLS as u16, 1)) && cglyphs(&b, &cid).contains(&(2 * WALL_COLS as u16, 1)), "an event from any room of the community counts");
}
