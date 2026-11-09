//! The shared wall: merge rules, generation (clear), lock, validation, snapshots, persistence, and the extended style document's
//! compatibility with documents and apps that know nothing about it.
mod common;
use common::*;
use commons_core::envelope::*;
use commons_core::wall::*;
use commons_proto::now_secs;

fn trio() -> (Net, Peer, Peer, Peer, String, String) {
    let net = Net::new();
    let (mut a, mut b, mut c) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"), Peer::new(&net, "carol"));
    let (cid, gen) = community_with(&net, &mut a, &mut [&mut b, &mut c]);
    (net, a, b, c, cid, gen)
}
fn acct(n: u8) -> String { format!("{:02x}", n).repeat(32) }
fn cells(p: &mut Peer, gid: &str) -> Vec<(u16, u8, u8, String)> { p.c.wall_view(gid).cells }
fn glyphs(p: &mut Peer, gid: &str) -> Vec<(u16, u8)> { cells(p, gid).into_iter().filter(|c| c.1 != 0).map(|c| (c.0, c.1)).collect() }

// ------------------------------------------------------------------------------------------------ the style document
/// What a phase-3 (or older) app reads of the document.
#[derive(serde::Deserialize, Debug)]
#[allow(dead_code)]
struct OldStyleDoc { ver: u64, ts: u64, #[serde(default)] community: Option<Duo>, #[serde(default)] rooms: std::collections::BTreeMap<String, Duo> }

#[test]
fn the_extended_style_document_is_compatible_both_ways() {
    let key = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]);
    let owner = hex::encode(key.verifying_key().to_bytes());
    let room = "abcdef0123456789".to_string();
    // A document without the new fields prints exactly as before (same bytes, so the same signature input), and still verifies.
    let plain = StyleDoc { ver: 3, ts: 7, ..Default::default() };
    let signed = sign_style(&key, "cc", &plain).unwrap();
    assert_eq!(signed.doc, "{\"ver\":3,\"ts\":7,\"community\":null,\"rooms\":{}}");
    assert_eq!(verify_style(&owner, "cc", &signed).unwrap(), plain);
    // An old signed document (no `locked` / `wall` at all) parses into the new type with the defaults.
    let old_text = "{\"ver\":1,\"ts\":2,\"community\":null,\"rooms\":{}}".to_string();
    let sig = {
        use ed25519_dalek::Signer;
        hex::encode(key.sign(&style_msg("cc", &old_text)).to_bytes())
    };
    let d = verify_style(&owner, "cc", &SignedStyle { doc: old_text, sig }).unwrap();
    assert!(d.locked.is_empty() && d.wall.is_empty());
    // A document with them verifies, round-trips, and an old reader (which ignores unknown fields) still reads the colours.
    let full = StyleDoc { ver: 4, ts: 9, locked: [room.clone()].into(), wall: [(room.clone(), 2)].into(), rooms: [(room.clone(), Duo { main: "#112233".into(), hi: "#445566".into() })].into(), ..Default::default() };
    let s = sign_style(&key, "cc", &full).unwrap();
    assert_eq!(verify_style(&owner, "cc", &s).unwrap(), full);
    let old: OldStyleDoc = serde_json::from_str(&s.doc).unwrap();
    assert_eq!(old.ver, 4);
    assert_eq!(old.rooms[&room].main, "#112233");
    // Bad values cannot be signed or believed.
    for bad in [StyleDoc { ver: 1, locked: ["zz".to_string()].into(), ..Default::default() }, StyleDoc { ver: 1, wall: [("nothex!!".to_string(), 1)].into(), ..Default::default() },
                StyleDoc { ver: 1, wall: [(room.clone(), 1 << 54)].into(), ..Default::default() }] {
        assert!(sign_style(&key, "cc", &bad).is_err(), "{bad:?}");
    }
    let other = ed25519_dalek::SigningKey::from_bytes(&[8u8; 32]);
    assert!(verify_style(&owner, "cc", &sign_style(&other, "cc", &full).unwrap()).is_err(), "only the owner's signature counts, lock and clear included");
    let mut tampered = s.clone(); tampered.doc = tampered.doc.replace("\"wall\":{\"abcdef0123456789\":2}", "\"wall\":{\"abcdef0123456789\":1}");
    assert_ne!(tampered.doc, s.doc);
    assert!(verify_style(&owner, "cc", &tampered).is_err(), "a lowered generation breaks the signature");
}

// ------------------------------------------------------------------------------------------------ merge
#[test]
fn merging_converges_whatever_order_batches_arrive_in() {
    // Batches from three accounts with overlapping cells, ties on the same clock included.
    let (a, b, c) = (acct(1), acct(2), acct(3));
    let batches: Vec<(u64, &String, Vec<(u32, u8, u8)>)> = vec![
        (1, &a, vec![(10, 1, 0), (11, 2, 0), (12, 3, 1)]),
        (1, &b, vec![(10, 4, 1), (13, 5, 1)]),
        (2, &c, vec![(11, 6, 1), (12, 0, 0)]),
        (2, &a, vec![(12, 7, 0), (14, 1, 1)]),
        (3, &b, vec![(10, 0, 0), (15, 2, 1)]),
        (3, &c, vec![(15, 3, 0), (14, 0, 0)]),
        (2, &b, vec![(13, 8, 0)]),
    ];
    let run = |order: &[usize]| {
        let mut w = Wall::default();
        for &i in order { let (k, who, cs) = &batches[i]; for &(p, g, col) in cs { w.put(p, g, col, *k, who); } }
        let mut v: Vec<(u16, u8, u8, u64, String)> = w.cells.iter().map(|(p, c)| (*p, c.0, c.1, c.2, w.author(c).to_string())).collect();
        v.sort(); v
    };
    let base = run(&[0, 1, 2, 3, 4, 5, 6]);
    // Every permutation of seven batches (5040) ends in the same picture, and applying one twice changes nothing.
    fn perms(n: usize) -> Vec<Vec<usize>> { if n == 0 { return vec![vec![]]; } let mut out = vec![]; for p in perms(n - 1) { for i in 0..=p.len() { let mut q = p.clone(); q.insert(i, n - 1); out.push(q); } } out }
    for p in perms(7) { assert_eq!(run(&p), base, "order {p:?}"); }
    let mut twice: Vec<usize> = (0..7).collect(); twice.extend(0..7);
    assert_eq!(run(&twice), base);
    // The higher clock wins, and on a tie the higher account.
    assert_eq!(base.iter().find(|c| c.0 == 10).map(|c| (c.1, c.4.clone())), Some((0, b.clone())), "clock 3 erased it");
    assert_eq!(base.iter().find(|c| c.0 == 13).map(|c| c.1), Some(8), "clock 2 beats clock 1");
    let mut w = Wall::default();
    w.put(1, 1, 0, 5, &a); w.put(1, 2, 0, 5, &b);
    assert_eq!(w.cells[&1].0, 2, "equal clocks: the higher account wins");
    let mut w2 = Wall::default();
    w2.put(1, 2, 0, 5, &b); w2.put(1, 1, 0, 5, &a);
    assert_eq!(w2.cells, { let mut x = w.cells.clone(); x.iter_mut().for_each(|(_, c)| c.3 = 0); x }, "same cell either way round (account index aside)");
}

#[test]
fn every_part_of_a_stroke_is_checked() {
    let mut w = Wall::default();
    let a = acct(1);
    for (pos, g, c, k) in [(WALL_CELLS, 1u8, 0u8, 1u64), (u32::MAX, 1, 0, 1), (0, 12, 0, 1), (0, 200, 0, 1), (0, 1, 2, 1), (0, 1, 0, 0), (0, 1, 0, (1 << 40) + 1)] {
        assert!(!w.put(pos, g, c, k, &a), "{pos} {g} {c} {k}");
    }
    assert!(!w.put(0, 1, 0, 1, "not-an-account"));
    assert!(w.cells.is_empty() && w.clock == 0);
    assert!(w.put(WALL_CELLS - 1, 11, 1, 1, &a), "the last cell, the last glyph, the highlight colour");
    for (g, ch) in WALL_GLYPHS.iter().enumerate().skip(1) { assert!(ch.is_ascii_graphic() && !matches!(ch, '<' | '>' | '&' | '"' | '\'' | '\\'), "glyph {g} {ch:?}"); }
}

// ------------------------------------------------------------------------------------------------ over the wire
#[test]
fn strokes_reach_friends_and_bad_ones_are_dropped_whole() {
    let (_net, mut a, mut b, mut c, _cid, gid) = trio();
    settle(&mut [&mut a, &mut b, &mut c]);
    let msgs = b.c.messages(&gid).len();
    assert!(a.c.wall_draw(&gid, &[(3, 4, 1, 0), (4, 4, 2, 1)]).unwrap());
    settle(&mut [&mut a, &mut b, &mut c]);
    let want = vec![(4 * WALL_COLS as u16 + 3, 1u8), (4 * WALL_COLS as u16 + 4, 2)];
    for p in [&mut a, &mut b, &mut c] { assert_eq!(glyphs(p, &gid), want, "{}", p.name); }
    // Not history, not a message, no notice.
    assert!(b.c.live_poll(0, None).is_empty(), "wall events never appear on the live feed");
    assert_eq!(b.c.messages(&gid).len(), msgs, "no message");
    assert!(!notices(&b).iter().any(|n| n.contains("newer")));
    // Our own client refuses bad strokes outright.
    assert!(a.c.wall_draw(&gid, &[]).is_err());
    assert!(a.c.wall_draw(&gid, &vec![(1, 1, 1, 0); WALL_BATCH + 1]).is_err());
    assert!(a.c.wall_draw(&gid, &[(WALL_COLS, 0, 1, 0)]).is_err());
    assert!(a.c.wall_draw(&gid, &[(0, WALL_ROWS, 1, 0)]).is_err());
    assert!(a.c.wall_draw(&gid, &[(0, 0, 12, 0)]).is_err());
    assert!(a.c.wall_draw(&gid, &[(0, 0, 1, 2)]).is_err());
    assert!(a.c.wall_draw("nonsense", &[(0, 0, 1, 0)]).is_err());
    assert!(a.c.wall_draw(&gid, &vec![(1, 1, 1, 0); WALL_BATCH]).is_ok(), "200 cells is the limit and fits");
    // Strokes are never silently replaced: when one is still waiting or the second's four are used, the next says "later" and changes nothing.
    let mut sent = 0;
    for i in 0..8u32 { let before = a.c.wall_view(&gid).rev; if a.c.wall_draw(&gid, &[(20 + i, 20, 1, 0)]).unwrap() { sent += 1; } else { assert_eq!(a.c.wall_view(&gid).rev, before, "a refused stroke leaves the wall alone"); } }
    assert!(sent <= 4, "at most four strokes a second leave ({sent})");
    // A hostile client sends bad ones straight into the live channel: every one is dropped whole, the good one next to it is kept.
    let me = now_secs();
    let send = |p: &mut Peer, data: String| p.c.debug_send_envelope(&gid, Envelope::Live { kind: "wall.d".into(), data, ts: me }).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    let before = glyphs(&mut c, &gid);
    send(&mut b, format!("{{\"g\":0,\"k\":9,\"c\":[[1,2],[{},2]]}}", WALL_CELLS));          // one cell off the grid
    send(&mut b, "{\"g\":0,\"k\":9,\"c\":[[1,2],[2,99]]}".into());                           // a glyph that does not exist
    send(&mut b, "{\"g\":0,\"k\":0,\"c\":[[1,2]]}".into());                                  // clock 0
    send(&mut b, "{\"g\":0,\"k\":9,\"c\":[]}".into());                                       // empty
    settle(&mut [&mut a, &mut b, &mut c]);
    std::thread::sleep(std::time::Duration::from_millis(1100));   // a receiver takes at most 8 a second from one sender
    send(&mut b, "{\"g\":0,\"k\":9,\"c\":[[-1,2]]}".into());                                 // negative
    send(&mut b, "{\"g\":0,\"k\":9,\"c\":[[1.5,2]]}".into());                                // not a whole number
    send(&mut b, "{\"g\":0,\"k\":9,\"c\":[[1,2,3]]}".into());                                // wrong shape
    send(&mut b, format!("{{\"g\":0,\"k\":9,\"c\":[{}]}}", vec!["[1,2]"; WALL_BATCH + 1].join(",")));   // 201 cells
    settle(&mut [&mut a, &mut b, &mut c]);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    send(&mut b, "not json".into());
    send(&mut b, "{\"g\":5,\"k\":9,\"c\":[[1,2]]}".into());                                  // another generation
    settle(&mut [&mut a, &mut b, &mut c]);
    assert_eq!(glyphs(&mut c, &gid), before, "none of the bad strokes changed anything");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    send(&mut b, "{\"g\":0,\"k\":9,\"c\":[[1,2],[2,3]]}".into());
    settle(&mut [&mut a, &mut b, &mut c]);
    assert!(glyphs(&mut c, &gid).contains(&(1, 1)) && glyphs(&mut c, &gid).contains(&(2, 1)), "a good one still lands");
}

#[test]
fn clearing_raises_the_generation_and_an_offline_device_cannot_bring_the_wall_back() {
    let (_net, mut a, mut b, mut c, cid, gid) = trio();
    settle(&mut [&mut a, &mut b, &mut c]);
    a.c.wall_draw(&gid, &[(1, 1, 1, 0), (2, 1, 1, 0)]).unwrap();
    b.c.wall_draw(&gid, &[(3, 1, 2, 1)]).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    assert_eq!(glyphs(&mut c, &gid).len(), 3);
    // Only the owner can clear or lock.
    assert!(b.c.set_wall(&cid, &gid, None, true).is_err() && b.c.set_wall(&cid, &gid, Some(true), false).is_err());
    assert!(a.c.set_wall(&cid, "deadbeefdeadbeef", None, true).is_err(), "not a room of this community");
    // Carol goes offline, the owner clears, bob and the owner see it.
    a.c.set_wall(&cid, &gid, None, true).unwrap();
    settle(&mut [&mut a, &mut b]);
    assert!(glyphs(&mut a, &gid).is_empty() && glyphs(&mut b, &gid).is_empty());
    assert_eq!(a.c.wall_view(&gid).gen, 1);
    // Carol still shows the old picture and draws on it, as an offline device would.
    assert_eq!(glyphs(&mut c, &gid).len(), 3);
    c.c.wall_draw(&gid, &[(9, 9, 5, 1)]).unwrap();
    let carol_batch = c.c.wall_snapshot(&gid).unwrap();
    assert!(!carol_batch.is_empty());
    settle(&mut [&mut a, &mut b, &mut c]);
    // She caught up: the clear reached her and her old stroke did not reach anyone.
    for p in [&mut a, &mut b, &mut c] { assert!(glyphs(p, &gid).is_empty(), "{} has an empty wall", p.name); assert_eq!(p.c.wall_view(&gid).gen, 1, "{}", p.name); }
    // Her old snapshot (generation 0) is ignored as well, sent late by anyone.
    for chunk in carol_batch { c.c.debug_send_envelope(&gid, Envelope::Live { kind: "wall.s".into(), data: chunk, ts: now_secs() }).unwrap(); }
    settle(&mut [&mut a, &mut b, &mut c]);
    assert!(glyphs(&mut a, &gid).is_empty() && glyphs(&mut b, &gid).is_empty());
    // New strokes of the new generation work, and the generation survives a restart.
    b.c.wall_draw(&gid, &[(5, 5, 3, 0)]).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    assert_eq!(glyphs(&mut a, &gid), vec![(5 * WALL_COLS as u16 + 5, 3)]);
    a.c.debug_flush_walls();
    a.c.save().unwrap();
}

#[test]
fn a_device_that_was_offline_for_a_clear_gets_it_when_it_returns() {
    let (_net, mut a, mut b, mut c, cid, gid) = trio();
    settle(&mut [&mut a, &mut b, &mut c]);
    a.c.wall_draw(&gid, &[(1, 1, 1, 0)]).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    assert_eq!(glyphs(&mut c, &gid).len(), 1);
    // Carol is not ticked at all while the owner clears and draws again; the clear is long gone from the live feed by the time she looks.
    a.c.set_wall(&cid, &gid, None, true).unwrap();
    settle(&mut [&mut a, &mut b]);
    a.c.wall_draw(&gid, &[(2, 2, 2, 1)]).unwrap();
    settle(&mut [&mut a, &mut b]);
    assert_eq!(glyphs(&mut c, &gid).len(), 1, "still the old picture while she is away");
    settle(&mut [&mut a, &mut b, &mut c]);
    assert_eq!(c.c.wall_view(&gid).gen, 1);
    let g = glyphs(&mut c, &gid);
    assert!(!g.contains(&(WALL_COLS as u16 + 1, 1)), "the cleared drawing is gone: {g:?}");
}

#[test]
fn a_locked_wall_takes_strokes_from_the_owner_only() {
    let (_net, mut a, mut b, mut c, cid, gid) = trio();
    settle(&mut [&mut a, &mut b, &mut c]);
    b.c.wall_draw(&gid, &[(1, 1, 1, 0)]).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    a.c.set_wall(&cid, &gid, Some(true), false).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    for p in [&mut a, &mut b, &mut c] { assert!(p.c.wall_view(&gid).locked, "{}", p.name); }
    // Bob's own client refuses; a forced stroke is dropped by both others.
    assert!(b.c.wall_draw(&gid, &[(2, 2, 2, 0)]).is_err());
    b.c.debug_send_envelope(&gid, Envelope::Live { kind: "wall.d".into(), data: "{\"g\":0,\"k\":50,\"c\":[[100,2]]}".into(), ts: now_secs() }).unwrap();
    // ... and so is a snapshot that tries to overwrite a cell
    let snap = format!("{{\"g\":0,\"i\":0,\"n\":1,\"a\":[\"{}\"],\"c\":[[{},4,99,0],[200,2,99,0]]}}", b.acct(), 1 * WALL_COLS + 1);
    b.c.debug_send_envelope(&gid, Envelope::Live { kind: "wall.s".into(), data: snap, ts: now_secs() }).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    for p in [&mut a, &mut c] {
        let g = glyphs(p, &gid);
        assert!(!g.iter().any(|x| x.0 == 100), "{}: the forced stroke was ignored", p.name);
        assert!(g.contains(&(WALL_COLS as u16 + 1, 1)), "{}: the existing cell was not overwritten by a snapshot", p.name);
    }
    // The owner still draws.
    a.c.wall_draw(&gid, &[(7, 7, 6, 1)]).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    assert!(glyphs(&mut c, &gid).iter().any(|x| x.0 == 7 * WALL_COLS as u16 + 7));
    // Unlocking lets members draw again, lock and clear survive a restart, and a non-owner's lock is refused everywhere.
    a.c.set_wall(&cid, &gid, Some(false), false).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    b.c.wall_draw(&gid, &[(2, 2, 2, 0)]).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    assert!(glyphs(&mut c, &gid).iter().any(|x| x.0 == 2 * WALL_COLS as u16 + 2));
    a.c.set_wall(&cid, &gid, Some(true), true).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    c.reopen(&_net);
    assert!(c.c.wall_view(&gid).locked && c.c.wall_view(&gid).gen == 1, "lock and generation are in the saved community table");
}

#[test]
fn a_late_joiner_gets_the_wall_from_one_member_in_at_most_eight_events() {
    let (net, mut a, mut b, mut c, cid, gid) = trio();
    settle(&mut [&mut a, &mut b, &mut c]);
    // A big wall drawn by three people: well over what eight events can carry.
    for who in 0..3u32 {
        let all: Vec<(u32, u32, u8, u8)> = (0..18u32).flat_map(|row| (0..WALL_COLS).map(move |x| { let y = row * 3 + who; (x, y, 1 + ((x + y) % 11) as u8, (x % 2) as u8) })).collect();
        for ch in all.chunks(WALL_BATCH) {
            let p = [&mut a, &mut b, &mut c].into_iter().nth(who as usize).unwrap();
            assert!(p.c.wall_draw(&gid, ch).unwrap());
            settle(&mut [&mut a, &mut b, &mut c]);
            std::thread::sleep(std::time::Duration::from_millis(270));   // the UI draws at most twice a second; the core allows four
        }
    }
    let total = glyphs(&mut a, &gid).len();
    assert_eq!(total, 18 * 3 * WALL_COLS as usize);
    let chunks = b.c.wall_snapshot(&gid).unwrap();
    assert!(!chunks.is_empty() && chunks.len() <= WALL_SNAP_CHUNKS, "{} chunks", chunks.len());
    for ch in &chunks { assert!(ch.len() <= MAX_LIVE, "a chunk of {} bytes", ch.len()); Envelope::parse(&Envelope::Live { kind: "wall.s".into(), data: ch.clone(), ts: 1 }.to_bytes()).unwrap(); }
    // Dave joins and starts with nothing; bob answers with his chunks, sent slowly (one a time here).
    let mut d = Peer::new(&net, "dave");
    let link = a.c.create_invite(&cid, None, 3600).unwrap();
    d.c.join_with_link(&link).unwrap();
    settle(&mut [&mut a, &mut b, &mut c, &mut d]);
    assert!(glyphs(&mut d, &gid).is_empty());
    for ch in &chunks { assert!(b.c.live_send(&gid, "wall.s", ch).unwrap()); settle(&mut [&mut a, &mut b, &mut c, &mut d]); }
    let got = glyphs(&mut d, &gid);
    assert!(got.len() >= 800 && got.len() <= total, "a snapshot carries what fits: {} of {total}", got.len());
    let theirs: std::collections::BTreeMap<u16, u8> = glyphs(&mut a, &gid).into_iter().collect();
    assert!(got.iter().all(|(p, g)| theirs.get(p) == Some(g)), "everything in the snapshot is what the others have");
    assert!(d.c.wall_view(&gid).snap_at > 0, "the arrival of a snapshot is noted, so others can stay quiet");
    // The other members' walls were not disturbed by receiving a snapshot of what they already have.
    assert_eq!(glyphs(&mut c, &gid).len(), total);
}

#[test]
fn the_wall_is_saved_with_the_state_on_a_slow_debounce() {
    let (net, mut a, mut b, mut c, _cid, gid) = trio();
    settle(&mut [&mut a, &mut b, &mut c]);
    let saved = |p: &Peer| p.c.st.walls.get(&gid).map_or(0, |w| w.cells.len());
    b.c.wall_draw(&gid, &[(8, 8, 3, 1)]).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    assert_eq!(saved(&a), 1, "the first change is saved at once");
    std::thread::sleep(std::time::Duration::from_millis(300));
    b.c.wall_draw(&gid, &[(9, 8, 4, 0)]).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    assert_eq!(glyphs(&mut a, &gid).len(), 2, "the live wall has both");
    assert_eq!(saved(&a), 1, "the next change inside the debounce does not rewrite the saved state");
    // A flush puts it there, encrypted with the rest, and a restart still has it.
    a.c.debug_flush_walls();
    a.c.save().unwrap();
    assert_eq!(saved(&a), 2);
    a.reopen(&net);
    assert_eq!(glyphs(&mut a, &gid).len(), 2);
    // Nothing of it is readable in the data folder.
    for e in std::fs::read_dir(a.dir.path()).unwrap().flatten() {
        if e.path().is_file() { let bytes = std::fs::read(e.path()).unwrap(); assert!(!contains(&bytes, "\"cells\""), "{:?} holds the wall in plain text", e.path()); }
    }
}

#[test]
fn what_the_relay_sees_of_a_wall_is_what_it_sees_of_any_message() {
    let (net, mut a, mut b, mut c, _cid, gid) = trio();
    settle(&mut [&mut a, &mut b, &mut c]);
    let before = relay_entries(&net, &gid).len();
    a.c.wall_draw(&gid, &[(1, 1, 1, 0)]).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    assert_eq!(relay_entries(&net, &gid).len(), before + 1, "one stroke is one ordinary encrypted entry");
    assert!(!contains(&relay_bytes(&net), "wall.d"));
}
