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
    let offer = |_p: &Peer, n: u8, to: &str, nonce: &str, link: &str, ts: u64| Envelope::Direct { id: id(n), ts, to: to.into(), nonce: nonce.into(), link: link.into(), sealed: None };
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

/// Audit M8: the offer travels in a room others can read. Before 0.3.6 it carried the invite's secret key, so a third member could
/// sign a join and only the founder's up-to-date admin devices stood in the way. Now the key is sealed to the one person named.
#[test]
fn a_third_member_who_reads_the_offer_cannot_use_its_invite() {
    let net = Net::new();
    let (mut alice, mut bob, mut carol) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"), Peer::new(&net, "carol"));
    let (_club, _gen) = community_with(&net, &mut alice, &mut [&mut bob, &mut carol]);
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert!(alice.c.st.profiles[&bob.acct()].direct_sealed, "bob's profile says his app opens sealed offers");
    let started = alice.c.direct_call(&bob.acct()).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert_eq!(bob.c.direct_list().len(), 1, "bob opened the sealed secret and joined: {:?}", notices(&bob));
    // Carol saw the very same offer. Its link carries a throwaway key, and the sealed secret is not hers to open.
    let (link, sealed) = carol.c.debug_offers.last().cloned().expect("carol's app saw the offer");
    assert!(sealed.is_some(), "the secret travelled sealed");
    let l = InviteLink::decode(&link).unwrap();
    let real_sk = alice.c.st.communities[&started.cid].invite_sks.values().next().cloned().unwrap();
    assert_ne!(l.sk, real_sk, "the link in the room does not hold the invite's real key");
    carol.c.join_with_link(&link).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert!(carol.c.st.communities.get(&started.cid).is_none_or(|c| c.removed), "carol is not let in");
    assert!(notices(&alice).iter().any(|n| n.contains("bad invite signature")), "the founder saw a join it could not verify, not one it had to refuse by policy: {:?}", notices(&alice));
    assert_eq!(alice.c.group_accounts(&started.cid).len(), 2);
    // A peer whose app predates sealing is still called the old way: the key in the link, nothing sealed.
    alice.c.st.profiles.get_mut(&carol.acct()).unwrap().direct_sealed = false;
    let to_carol = alice.c.direct_call(&carol.acct()).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    let (link2, sealed2) = bob.c.debug_offers.last().cloned().unwrap();
    assert!(sealed2.is_none() && InviteLink::decode(&link2).unwrap().community == to_carol.cid, "an open offer, as before 0.3.6");
    assert_eq!(carol.c.direct_list().len(), 1, "and carol's app (which also knows the old form) joined");
}

/// Audit M8: a stranger who shares a room cannot fill the receiver's call slots with offers of their own communities.
#[test]
fn one_founder_cannot_fill_the_call_slots() {
    let net = Net::new();
    let (mut alice, mut bob) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (_club, gen) = community_with(&net, &mut alice, &mut [&mut bob]);
    settle(&mut [&mut alice, &mut bob]);
    // Alice founds direct communities for bob again and again (a changed app would), each with a fresh nonce, and offers each.
    for i in 0..4u8 {
        let nonce = new_nonce(&alice.acct(), &bob.acct());
        let cid = alice.c.debug_create_community_nonce("Direct call (open in a newer Megatron)", &nonce, "call", ROOM_VOICE).unwrap();
        settle(&mut [&mut alice, &mut bob]);
        let link = alice.c.create_invite(&cid, Some(2), 600).unwrap();
        let mut l = InviteLink::decode(&link).unwrap();
        l.secret.clear();
        let sealed = seal_for(&bob.acct(), &l.sk).unwrap();
        l.sk = "cd".repeat(32);
        alice.c.debug_send_envelope(&gen, Envelope::Direct { id: format!("{:032x}", 100 + i as u32), ts: now_secs(), to: bob.acct(), nonce, link: l.encode(), sealed: Some(sealed) }).unwrap();
        settle(&mut [&mut alice, &mut bob]);
        // offers from one sender are also rationed per minute: move the clock on
        bob.c.debug_clear_direct_rate();
    }
    assert_eq!(bob.c.direct_list().len(), DIRECT_MAX_PER_FOUNDER, "bob holds at most {} communities founded by one person: {:?}", DIRECT_MAX_PER_FOUNDER, bob.c.direct_list().len());
}
