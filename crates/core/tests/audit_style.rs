//! Audit finding: a plain member could put an older owner-signed style document back in the group table, so every device that
//! reads the table afresh (a new member, a re-added device) got old colours, the old wall lock state and the old wall generation.
mod common;
use common::*;
use commons_core::envelope::{sign_style, Duo};
use commons_core::state::Role;

fn duo(a: &str, b: &str) -> Duo { Duo { main: a.into(), hi: b.into() } }
fn room_colours(p: &Peer, cid: &str, gid: &str) -> Option<Duo> { p.c.st.communities[cid].style.as_ref().and_then(|s| s.rooms.get(gid).cloned()) }

/// Alice owns, Carol is an admin (admits newcomers while the owner is away), Bob is a plain member. The owner has locked and cleared
/// the wall, then unlocked and cleared again, and changed the colours twice. Returns the older document.
fn world() -> (Net, Peer, Peer, Peer, String, String, commons_core::envelope::StyleDoc) {
    let net = Net::new();
    let (mut a, mut b, mut c) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"), Peer::new(&net, "carol"));
    let (cid, gen) = community_with(&net, &mut a, &mut [&mut b, &mut c]);
    a.c.set_role(&cid, &c.acct(), Role::Admin).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    assert_eq!(a.c.st.communities[&cid].role_of(&b.acct()), Some(Role::Member));
    a.c.set_wall(&cid, &gen, Some(true), true).unwrap();
    a.c.set_style(&cid, Some(&gen), Some(duo("#111111", "#222222"))).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    let old = a.c.st.communities[&cid].style.clone().unwrap();
    a.c.set_wall(&cid, &gen, Some(false), true).unwrap();
    a.c.set_style(&cid, Some(&gen), Some(duo("#333333", "#444444"))).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    let cur = a.c.st.communities[&cid].style.clone().unwrap();
    assert!(cur.ver > old.ver && cur.wall.get(&gen) == Some(&2) && !cur.locked.contains(&gen));
    (net, a, b, c, cid, gen, old)
}

#[test]
#[ignore = "needs ENFORCE_STYLE_FORWARD, off until versions before the rule are gone"]
fn a_plain_member_cannot_roll_the_signed_style_back_for_newcomers() {
    let (net, mut a, mut b, mut c, cid, gen, old) = world();
    // The owner goes offline. Bob, a plain member, commits the table with the OLD owner-signed document put back.
    let replay = sign_style(&a.c.debug_account_key(), &cid, &old).unwrap();
    let _ = b.c.debug_plant_style(&cid, Some(replay));
    settle(&mut [&mut b, &mut c]);
    assert!(notices(&c).iter().any(|n| n.contains("only move forward")), "carol should refuse it, notices: {:?}", notices(&c));
    // A newcomer let in by the admin reads the table as it stands: the owner's current document.
    let mut d = Peer::new(&net, "dave");
    let link = c.c.create_invite(&cid, None, 3600).unwrap();
    d.c.join_with_link(&link).unwrap();
    settle(&mut [&mut b, &mut c, &mut d]);
    let ds = d.c.st.communities[&cid].style.clone().expect("dave has a style document");
    assert!(ds.ver > old.ver, "dave got the owner's current document (ver {}, old {})", ds.ver, old.ver);
    assert!(!ds.locked.contains(&gen) && ds.wall.get(&gen) == Some(&2));
    assert_eq!(room_colours(&d, &cid, &gen), Some(duo("#333333", "#444444")));
    assert!(d.c.wall_draw(&gen, &[(1, 1, 1, 0)]).is_ok(), "dave can draw on the unlocked wall");
    let _ = &mut a;
}

#[test]
fn a_dropped_document_is_put_back_by_the_owner_without_a_commit_storm() {
    let (net, mut a, mut b, mut c, cid, gen, _old) = world();
    let cur = a.c.st.communities[&cid].style.clone().unwrap();
    // What an older admin's app does when it renames or adds a room: rewrites the table without the field it does not know.
    // That must stay allowed (older clients keep working), and the owner repairs it at once.
    b.c.debug_plant_style(&cid, None).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    let after = a.c.st.communities[&cid].style.clone().unwrap();
    assert!(after.ver > cur.ver, "the owner republished (ver {} -> {})", cur.ver, after.ver);
    assert_eq!(a.c.st.communities[&cid].style_repub_gap, 30);
    // A second drop right away is not repaired right away: a hostile member cannot make the owner commit faster than the gap.
    let ver = after.ver;
    b.c.debug_plant_style(&cid, None).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    assert_eq!(a.c.st.communities[&cid].style.as_ref().unwrap().ver, ver, "no second commit inside the gap");
    // ...but it is repaired once the gap has passed (time is moved back instead of waiting), and the gap doubles.
    { let x = a.c.st.communities.get_mut(&cid).unwrap(); x.style_repub -= 40; }
    settle(&mut [&mut a, &mut b, &mut c]);
    assert!(a.c.st.communities[&cid].style.as_ref().unwrap().ver > ver, "repaired after the gap");
    assert_eq!(a.c.st.communities[&cid].style_repub_gap, 60);
    // A newcomer now gets the owner's document.
    let mut d = Peer::new(&net, "dave");
    let link = a.c.create_invite(&cid, None, 3600).unwrap();
    d.c.join_with_link(&link).unwrap();
    settle(&mut [&mut a, &mut b, &mut c, &mut d]);
    assert_eq!(room_colours(&d, &cid, &gen), Some(duo("#333333", "#444444")));
}

#[test]
fn a_member_may_still_carry_a_newer_owner_document_into_a_stale_room() {
    let (_net, mut a, mut b, mut c, cid, gen, _old) = world();
    let cur = a.c.st.communities[&cid].style.clone().unwrap();
    let mut newer = cur.clone();
    newer.ver += 5; newer.ts += 5;
    newer.rooms.insert(gen.clone(), duo("#555555", "#666666"));
    let signed = sign_style(&a.c.debug_account_key(), &cid, &newer).unwrap();
    b.c.debug_plant_style(&cid, Some(signed)).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    for p in [&a, &b, &c] { assert_eq!(room_colours(p, &cid, &gen), Some(duo("#555555", "#666666")), "{}", p.name); }
    assert!(!notices(&c).iter().any(|n| n.contains("only move forward") || n.contains("only the owner can change the colours")));
}

#[test]
#[ignore = "needs ENFORCE_STYLE_FORWARD, off until versions before the rule are gone"]
fn a_forged_document_in_the_table_is_refused() {
    let (_net, mut a, mut b, mut c, cid, gen, _old) = world();
    let cur = a.c.st.communities[&cid].style.clone().unwrap();
    let mut evil = cur.clone();
    evil.ver += 50;
    evil.rooms.insert(gen.clone(), duo("#ff0000", "#ff0000"));
    let forged = sign_style(&b.c.debug_account_key(), &cid, &evil).unwrap();
    let _ = b.c.debug_plant_style(&cid, Some(forged));
    settle(&mut [&mut a, &mut b, &mut c]);
    assert!(notices(&c).iter().any(|n| n.contains("only the owner can change the colours")), "{:?}", notices(&c));
    for p in [&a, &c] { assert_eq!(room_colours(p, &cid, &gen), Some(duo("#333333", "#444444")), "{}", p.name); }
}
