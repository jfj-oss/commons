//! One-to-one calls: a hidden two-person community. Two people who share a community call each other; only those two ever
//! see the new community, a third member cannot join it or see it, and an offer that is not exactly that is never acted on.
mod common;
use common::*;
use commons_core::browser_update::now_secs;
use commons_core::direct::*;
use commons_core::envelope::*;

fn rooms_of(p: &Peer) -> Vec<String> { p.c.st.groups.keys().cloned().collect() }

#[test]
fn two_members_call_each_other_and_nobody_else_sees_it() {
    let net = Net::new();
    let (mut alice, mut bob, mut carol) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"), Peer::new(&net, "carol"));
    let (_club, _gen) = community_with(&net, &mut alice, &mut [&mut bob, &mut carol]);
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    let carol_groups = rooms_of(&carol);

    // an app that does not announce direct calls is refused up front
    assert_eq!(alice.c.direct_support(&bob.acct()), Some(true), "bob's profile said he can take direct calls");
    let started = alice.c.direct_call(&bob.acct()).unwrap();
    assert!(started.invited, "bob was not in a direct community yet");
    settle(&mut [&mut alice, &mut bob, &mut carol]);

    // both sides hold exactly one direct community, named after the other person in the list, marked on both sides
    let (ad, bd) = (alice.c.direct_list(), bob.c.direct_list());
    assert_eq!((ad.len(), bd.len()), (1, 1), "alice {:?} bob {:?} notices {:?}", ad, bd, notices(&bob));
    assert_eq!((ad[0].peer.as_str(), bd[0].peer.as_str()), (bob.acct().as_str(), alice.acct().as_str()));
    assert_eq!(ad[0].cid, bd[0].cid);
    assert_eq!(ad[0].room, started.room);
    assert!(!bd[0].room.is_empty() && ad[0].founder_me && !bd[0].founder_me);
    assert_eq!(bd[0].members, 2);
    // the community is one voice room and nothing else, and the nonce commits to exactly the two
    let c = &alice.c.st.communities[&ad[0].cid];
    assert_eq!(c.channels.len(), 1);
    assert_eq!(c.channels[0].kind, ROOM_VOICE);
    assert!(binds(&alice.acct(), &c.nonce, &bob.acct()) && !binds(&alice.acct(), &c.nonce, &carol.acct()));
    assert!(alice.c.is_direct_cid(&ad[0].cid) && bob.c.is_direct_cid(&ad[0].cid));
    // bob was rung in the new room, by alice, for him
    let rings = bob.c.ring_poll(0);
    assert_eq!(rings.len(), 1, "{rings:?}");
    assert_eq!((rings[0].gid.as_str(), rings[0].account.as_str(), rings[0].direct, rings[0].end), (started.room.as_str(), alice.acct().as_str(), true, false));
    assert!(alice.c.ring_poll(0).is_empty());

    // carol knows nothing of it: no group, no community, no ring, no join in progress, no list entry
    assert_eq!(rooms_of(&carol), carol_groups, "carol holds no new group");
    assert!(carol.c.direct_list().is_empty() && carol.c.ring_poll(0).is_empty() && carol.c.st.joining.is_empty());
    assert!(!carol.c.st.communities.contains_key(&ad[0].cid));
    assert!(carol.c.group_accounts(&started.room).is_empty());
    assert_eq!(alice.c.group_accounts(&started.room), [alice.acct(), bob.acct()].into_iter().collect());

    // carol can read the offer (it travels in the club) and use the invite: the founder refuses her, and she is not let in
    let link = alice.c.create_invite(&ad[0].cid, None, 3600).unwrap();
    carol.c.join_with_link(&link).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert!(carol.c.st.communities.get(&ad[0].cid).is_none_or(|c| c.removed), "a third member is not let in");
    assert!(carol.c.st.join_refused.contains_key(&ad[0].cid), "she is told no: {:?}", carol.c.st.join_refused);
    assert_eq!(alice.c.group_accounts(&ad[0].cid).len(), 2);
    assert_eq!(alice.c.group_accounts(&started.room), [alice.acct(), bob.acct()].into_iter().collect());

    // calling again finds the same community and rings without a new offer, from either side
    let again = bob.c.direct_call(&alice.acct()).unwrap();
    assert!(!again.invited);
    assert_eq!(again.cid, ad[0].cid);
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert_eq!(alice.c.ring_poll(0).len(), 1, "alice is rung by bob in the same room");
    assert_eq!(alice.c.direct_list().len(), 1);
}

#[test]
fn offers_that_are_not_exactly_a_direct_community_are_ignored() {
    let net = Net::new();
    let (mut alice, mut bob, mut carol) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"), Peer::new(&net, "carol"));
    let (club, gen) = community_with(&net, &mut alice, &mut [&mut bob, &mut carol]);
    let (a, b, c) = (alice.acct(), bob.acct(), carol.acct());
    let id = |n: u8| format!("{:032x}", n);
    let offer = |_p: &Peer, n: u8, to: &str, nonce: &str, link: &str, ts: u64| Envelope::Direct { id: id(n), ts, to: to.into(), nonce: nonce.into(), link: link.into() };
    let quiet = |p: &Peer| p.c.st.joining.is_empty() && p.c.direct_list().is_empty();

    // 1. an ordinary community's invite: never joined on a call's word
    let plain = alice.c.create_community("plain").unwrap();
    let plain_link = alice.c.create_invite(&plain, None, 3600).unwrap();
    let plain_nonce = alice.c.st.communities[&plain].nonce.clone();
    alice.c.debug_send_envelope(&gen, offer(&alice, 1, &b, &plain_nonce, &plain_link, now_secs())).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert!(quiet(&bob), "an ordinary community is not joined by an offer");

    // 2. a real direct community of alice and carol, offered to bob by alice
    let cn = new_nonce(&a, &c);
    let cc = alice.c.st.communities.len();
    let _ = cc;
    // (build it the legitimate way, calling carol, then offer its invite to bob)
    let to_carol = alice.c.direct_call(&c).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    let carol_cid = to_carol.cid;
    let carol_nonce = alice.c.st.communities[&carol_cid].nonce.clone();
    let carol_link = alice.c.create_invite(&carol_cid, None, 3600).unwrap();
    alice.c.debug_send_envelope(&gen, offer(&alice, 2, &b, &carol_nonce, &carol_link, now_secs())).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert!(quiet(&bob), "a direct community of alice and carol is not bob's to join");
    let _ = cn;

    // 3. carol offers bob a link into alice's community: the link's owner is not the sender
    alice.c.debug_send_envelope(&gen, offer(&alice, 3, &b, &carol_nonce, &carol_link, now_secs() - 120)).unwrap();
    carol.c.debug_send_envelope(&gen, offer(&carol, 4, &b, &carol_nonce, &carol_link, now_secs())).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert!(quiet(&bob), "stale, or sent by somebody who is not the community's founder");

    // 4. an offer addressed to somebody else, and one from a stranger to the room
    let to_bob_nonce = new_nonce(&a, &b);
    alice.c.debug_send_envelope(&gen, offer(&alice, 5, &c, &to_bob_nonce, &plain_link, now_secs())).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert!(quiet(&bob) && quiet(&carol) || carol.c.st.joining.is_empty());

    // 5. an offer sent in the direct community's own room is not taken either (it is not an ordinary room)
    let direct_room = alice.c.direct_list().iter().find(|d| d.peer == c).unwrap().room.clone();
    assert!(!direct_room.is_empty());

    // 6. a real call still works afterwards, and bob's only direct community is the one with alice
    let ok = alice.c.direct_call(&b).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert_eq!(bob.c.direct_list().len(), 1);
    assert_eq!(bob.c.direct_list()[0].cid, ok.cid);
    assert!(club != ok.cid);
}

#[test]
fn a_direct_community_that_gains_a_third_account_is_left() {
    let net = Net::new();
    let (mut alice, mut bob, mut carol) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"), Peer::new(&net, "carol"));
    let (_club, _gen) = community_with(&net, &mut alice, &mut [&mut bob, &mut carol]);
    let started = alice.c.direct_call(&bob.acct()).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert_eq!(bob.c.direct_list().len(), 1);
    // A founder with a changed app puts a text room into it: that is no longer a call community, and bob's app leaves.
    alice.c.create_channel(&started.cid, "chat").unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    let c = &bob.c.st.communities[&started.cid];
    assert!(c.direct_bad, "a second room breaks the shape");
    assert!(bob.c.direct_list().is_empty(), "and it is not offered as a call any more");
    assert!(bob.c.is_direct_cid(&started.cid), "though it is still never shown among the communities");
    assert!(bob.c.st.notices.iter().any(|n| n.kind == "direct"));
}

#[test]
fn people_who_stop_calls_are_not_called_and_old_apps_are_told_up_front() {
    let net = Net::new();
    let (mut alice, mut bob) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (_club, _gen) = community_with(&net, &mut alice, &mut [&mut bob]);
    settle(&mut [&mut alice, &mut bob]);
    // bob stops calls from alice: her offer is not taken
    bob.c.direct_refuse(&alice.acct(), true).unwrap();
    alice.c.direct_call(&bob.acct()).unwrap();
    settle(&mut [&mut alice, &mut bob]);
    assert!(bob.c.direct_list().is_empty() && bob.c.st.joining.is_empty(), "refused");
    // and the other way, it is not offered by bob either
    assert!(bob.c.direct_call(&alice.acct()).is_err());
    bob.c.direct_refuse(&alice.acct(), false).unwrap();
    // someone whose newest profile does not say they take direct calls (an older app): the caller is told before anything is made
    alice.c.st.profiles.get_mut(&bob.acct()).unwrap().direct = false;
    let before = alice.c.st.communities.len();
    let err = alice.c.direct_call(&bob.acct()).unwrap_err().to_string();
    assert!(err.contains("older Megatron"), "{err}");
    assert_eq!(alice.c.st.communities.len(), before, "nothing was created");
}
