mod common;
use common::*;
use commons_core::client::Client;
use commons_core::vault::KdfParams;

fn peer_from(c: Client, dir: tempfile::TempDir, faults: std::sync::Arc<std::sync::Mutex<Faults>>, name: &str) -> Peer { Peer { name: name.into(), c, dir, faults } }

#[test]
fn second_device_by_pairing_code_and_revocation() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut bob = Peer::new(&net, "bob");
    let (cid, gen) = community_with(&net, &mut alice, &mut [&mut bob]);
    alice.c.send_text(&gen, "history before the second device", None).unwrap();
    settle(&mut [&mut alice, &mut bob]);

    // new laptop shows a code (QR); the phone-side device approves it
    let dir = tempfile::tempdir().unwrap();
    let (t, faults) = transport(&net.url());
    let (c2, code) = Client::create_for_pairing(dir.path(), "pw2", &net.url(), "", KdfParams::TEST, t).unwrap();
    let mut bob2 = peer_from(c2, dir, faults, "bob2");
    assert!(!bob2.c.has_identity());
    assert!(code.starts_with("commons-pair:"));
    // a code for a different relay is refused; garbage is refused
    assert!(bob.c.pairing_approve("commons-pair:AAAA").is_err());
    let check = bob.c.pairing_approve(&code).unwrap();
    assert_eq!(check.len(), 4);
    assert_eq!(Some(check.clone()), bob2.c.pairing_check_new(), "both screens show the same check digits");
    settle(&mut [&mut alice, &mut bob, &mut bob2]);
    assert!(bob2.c.has_identity());
    assert_eq!(bob2.acct(), bob.acct(), "same account, different device");
    assert_ne!(bob2.c.my_device(), bob.c.my_device());
    assert_eq!(alice.c.group_devices(&cid).len(), 3);
    assert_eq!(alice.c.group_devices(&gen).len(), 3);
    assert_eq!(bob2.c.display_name(&alice.acct()), "alice", "{:#?}\n{:#?}", notices(&bob2), notices(&alice));
    assert_eq!(alice.c.members(&cid).iter().find(|m| m.account == bob.acct()).unwrap().devices, 2);
    assert_eq!(bob2.c.channels_of(&cid).len(), 1);

    alice.c.send_text(&gen, "hello both of bob's devices", None).unwrap();
    bob2.c.send_text(&gen, "typed on the laptop", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut bob2]);
    assert!(has_text(&bob, &gen, "hello both of bob's devices") && has_text(&bob2, &gen, "hello both of bob's devices"));
    assert!(has_text(&alice, &gen, "typed on the laptop") && has_text(&bob, &gen, "typed on the laptop"));
    // honest limit, documented: the new device does not get old history
    assert!(!has_text(&bob2, &gen, "history before the second device"));
    assert_eq!(alice.c.epoch_fingerprint(&gen), bob2.c.epoch_fingerprint(&gen));

    // laptop is lost -> revoke from the other device; it is removed and can't read new messages
    let lost = bob2.c.my_device();
    bob.c.revoke_device(&lost).unwrap();
    settle(&mut [&mut alice, &mut bob]);
    assert_eq!(alice.c.group_devices(&gen).len(), 2);
    assert!(!alice.c.group_devices(&cid).contains(&lost));
    alice.c.send_text(&gen, "after revocation", None).unwrap();
    settle(&mut [&mut alice, &mut bob]);
    for (_s, d) in relay_entries(&net, &gen) { if let Ok(pt) = bob2.c.debug_try_decrypt(&gen, &d) { assert!(!String::from_utf8_lossy(&pt).contains("after revocation")); } }
    // the relay refuses the revoked device outright
    assert!(bob2.c.tick(0).is_err());
    assert!(has_text(&bob, &gen, "after revocation"));
}

#[test]
fn recovery_key_restores_identity_not_history() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut bob = Peer::new(&net, "bob");
    let mut carol = Peer::new(&net, "carol");
    let (cid, gen) = community_with(&net, &mut alice, &mut [&mut bob, &mut carol]);
    alice.c.send_text(&gen, "old secret history", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    let key = carol.c.recovery_key().unwrap();
    assert!(Client::create_from_recovery(tempfile::tempdir().unwrap().path(), "x", &net.url(), "c", "AAAA-BBBB", KdfParams::TEST, transport(&net.url()).0).is_err(), "bad key rejected");
    let mut typo = key.clone().into_bytes(); typo[2] = if typo[2] == b'A' { b'B' } else { b'A' };
    assert!(Client::create_from_recovery(tempfile::tempdir().unwrap().path(), "x", &net.url(), "c", &String::from_utf8(typo).unwrap(), KdfParams::TEST, transport(&net.url()).0).is_err(), "typo caught by checksum");

    // carol's laptop is gone. New machine + recovery key.
    let dir = tempfile::tempdir().unwrap();
    let (t, f) = transport(&net.url());
    let c = Client::create_from_recovery(dir.path(), "newpw", &net.url(), "carol", &key, KdfParams::TEST, t).unwrap();
    let mut carol2 = peer_from(c, dir, f, "carol2");
    assert_eq!(carol2.acct(), carol.acct());
    settle(&mut [&mut alice, &mut bob, &mut carol2]);
    assert!(carol2.c.group_devices(&gen).contains(&carol2.c.my_device()), "{:#?} {:#?} {:#?}", notices(&carol2), notices(&alice), carol2.c.my_devices().map(|d| d.len()));
    assert!(!has_text(&carol2, &gen, "old secret history"), "recovery key restores identity, not message history");
    // the old (stolen/lost) device must be revoked; until then it is still a member
    let old = carol.c.my_device();
    assert!(alice.c.group_devices(&gen).contains(&old));
    let devs = carol2.c.my_devices().unwrap();
    assert_eq!(devs.len(), 2);
    carol2.c.revoke_device(&old).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol2]);
    assert!(!alice.c.group_devices(&gen).contains(&old));
    bob.c.send_text(&gen, "after recovery", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol2]);
    assert!(has_text(&carol2, &gen, "after recovery"));
    let _ = cid;
}

#[test]
fn leave_and_rename() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut bob = Peer::new(&net, "bob");
    let mut carol = Peer::new(&net, "carol");
    let (cid, gen) = community_with(&net, &mut alice, &mut [&mut bob, &mut carol]);
    assert!(alice.c.leave(&cid).is_err(), "owner cannot leave");
    bob.c.leave(&cid).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert_eq!(alice.c.group_accounts(&gen).len(), 2);
    assert!(bob.c.st.communities[&cid].removed);
    alice.c.send_text(&gen, "bob is gone", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert!(!has_text(&bob, &gen, "bob is gone"));
    assert!(has_text(&carol, &gen, "bob is gone"));
    alice.c.rename_community(&cid, "club two").unwrap();
    settle(&mut [&mut alice, &mut carol]);
    assert_eq!(carol.c.st.communities[&cid].name, "club two");
    assert!(carol.c.rename_community(&cid, "x").is_err());
}

/// A second device of `p`, approved from `p`. Everything else has to be settled by the caller.
fn pair(net: &Net, p: &mut Peer, others: &mut [&mut Peer], name: &str) -> Peer {
    let dir = tempfile::tempdir().unwrap();
    let (t, faults) = transport(&net.url());
    let (c, code) = Client::create_for_pairing(dir.path(), "pw2", &net.url(), "", KdfParams::TEST, t).unwrap();
    let mut new = peer_from(c, dir, faults, name);
    impatient(&mut new.c);
    p.c.pairing_approve(&code).unwrap();
    let mut all: Vec<&mut Peer> = vec![&mut *p, &mut new];
    for o in others.iter_mut() { all.push(&mut **o); }
    settle(&mut all);
    assert!(new.c.has_identity());
    new
}

#[test]
fn two_devices_are_not_a_way_to_pull_someone_into_a_community() {
    let net = Net::new();
    let mut m = Peer::new(&net, "mallory");
    let mut bob = Peer::new(&net, "bob");
    let mut bob2 = pair(&net, &mut bob, &mut [&mut m], "bob2");
    let cid = m.c.create_community("evil").unwrap();
    let room = m.c.channels_of(&cid)[0].gid.clone();
    settle(&mut [&mut m, &mut bob, &mut bob2]);
    // Whoever runs a community can put every device of an account in one commit, so each one sees the other in the group.
    m.c.debug_add_account_devices(&cid, &bob.acct()).unwrap();
    m.c.debug_add_account_devices(&room, &bob.acct()).unwrap();
    assert_eq!(m.c.group_devices(&cid).len(), 3, "the attack really happened: {:?}", notices(&m));
    settle(&mut [&mut m, &mut bob, &mut bob2]);
    for p in [&bob, &bob2] {
        assert!(p.c.st.communities.is_empty() && p.c.st.groups.is_empty(), "{} was pulled in: {:?}", p.name, notices(p));
        assert!(notices(p).iter().any(|n| n.contains("unsolicited")), "{:?}", notices(p));
    }
}

#[test]
fn a_consent_counts_for_one_account_in_one_community() {
    use commons_core::envelope::*;
    let k = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let (a, b) = ("ab".repeat(32), "cd".repeat(32));
    let c = sign_consent(&k, &a, 5);
    assert!(verify_consent(&a, &c).is_ok());
    assert!(verify_consent(&b, &c).is_err(), "another community");
    assert!(verify_consent(&a, &Consent { ts: 6, ..c.clone() }).is_err(), "another time");
    assert!(verify_consent(&a, &Consent { account: hex::encode(ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng).verifying_key().to_bytes()), ..c.clone() }).is_err(), "another account");
    let mut list = vec![];
    assert!(merge_consent(&mut list, c.clone()) && !merge_consent(&mut list, c.clone()));
    assert!(merge_consent(&mut list, sign_consent(&k, &a, 7)) && list.len() == 1 && list[0].ts == 7);
    assert!(!merge_consent(&mut list, sign_consent(&k, &a, 6)), "an older one does not win");
}

#[test]
fn a_community_left_cannot_take_a_person_back_with_the_old_consent() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut bob = Peer::new(&net, "bob");
    let (cid, gen) = community_with(&net, &mut alice, &mut [&mut bob]);
    let mut bob2 = pair(&net, &mut bob, &mut [&mut alice], "bob2");
    settle(&mut [&mut alice, &mut bob, &mut bob2]);
    assert_eq!(alice.c.group_devices(&cid).len(), 3);
    let old = alice.c.debug_consents(&cid).into_iter().find(|c| c.account == bob.acct()).expect("the table holds bob's consent");
    bob.c.leave(&cid).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut bob2]);
    assert!(bob.c.st.communities[&cid].removed && bob2.c.st.communities[&cid].removed);
    assert!(!alice.c.debug_consents(&cid).iter().any(|c| c.account == bob.acct()), "an honest owner drops it");

    // Now the owner turns hostile: the signature is genuine, and this is the right community.
    alice.c.debug_plant_consent(&cid, old.clone()).unwrap();
    alice.c.debug_add_account_devices(&cid, &bob.acct()).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut bob2]);
    assert!(bob.c.st.communities[&cid].removed && bob2.c.st.communities[&cid].removed, "{:?}", notices(&bob));

    // A device paired after the leaving was told, so it refuses too.
    let mut bob3 = pair(&net, &mut bob, &mut [&mut alice], "bob3");
    settle(&mut [&mut alice, &mut bob, &mut bob3]);
    assert!(alice.c.group_devices(&cid).contains(&bob3.c.my_device()), "the owner, seeing bob in the group, added it: {:?}", notices(&alice));
    assert!(bob3.c.st.communities.is_empty(), "{:?}", notices(&bob3));

    // Coming back on purpose works, and takes the other devices along: that consent is a new one.
    let mut alice2 = Peer::new(&net, "alice-fresh");
    let cid2 = alice2.c.create_community("fresh").unwrap();
    let gen2 = alice2.c.channels_of(&cid2)[0].gid.clone();
    let link = alice2.c.create_invite(&cid2, Some(1), 3600).unwrap();
    bob.c.join_with_link(&link).unwrap();
    settle(&mut [&mut alice2, &mut bob, &mut bob2, &mut bob3]);
    for p in [&bob, &bob2, &bob3] { assert!(p.c.st.communities.get(&cid2).is_some_and(|c| !c.removed), "{}: {:?}", p.name, notices(p)); }
    assert_eq!(alice2.c.group_devices(&gen2).len(), 4);
    let _ = gen;
}

#[test]
fn the_same_community_can_be_rejoined_after_leaving() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut bob = Peer::new(&net, "bob");
    let (cid, gen) = community_with(&net, &mut alice, &mut [&mut bob]);
    let mut bob2 = pair(&net, &mut bob, &mut [&mut alice], "bob2");
    bob.c.leave(&cid).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut bob2]);
    assert!(bob2.c.st.communities[&cid].removed);
    let link = alice.c.create_invite(&cid, Some(1), 3600).unwrap();
    bob.c.join_with_link(&link).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut bob2]);
    assert!(!bob.c.st.communities[&cid].removed && !bob2.c.st.communities[&cid].removed, "{:?}", notices(&bob2));
    alice.c.send_text(&gen, "welcome back", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut bob2]);
    assert!(has_text(&bob, &gen, "welcome back") && has_text(&bob2, &gen, "welcome back"));
}

#[test]
fn a_cancelled_join_stays_cancelled() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut bob = Peer::new(&net, "bob");
    let cid = alice.c.create_community("club").unwrap();
    let link = alice.c.create_invite(&cid, None, 3600).unwrap();
    settle(&mut [&mut alice, &mut bob]);
    bob.c.join_with_link(&link).unwrap();
    bob.tick(); // the request, with the consent in it, is on its way
    bob.c.cancel_join(&cid).unwrap();
    settle(&mut [&mut alice, &mut bob]);
    assert!(!bob.c.st.communities.get(&cid).is_some_and(|c| !c.removed), "{:?}", notices(&bob));
}

#[test]
fn the_owner_and_a_member_each_bring_a_new_device_in() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut bob = Peer::new(&net, "bob");
    let (cid, gen) = community_with(&net, &mut alice, &mut [&mut bob]);
    let mut alice2 = pair(&net, &mut alice, &mut [&mut bob], "alice2");
    assert!(alice2.c.st.communities.get(&cid).is_some_and(|c| !c.removed), "{:?}", notices(&alice2));
    alice2.c.send_text(&gen, "from the owner's new device", None).unwrap();
    settle(&mut [&mut alice, &mut alice2, &mut bob]);
    assert!(has_text(&bob, &gen, "from the owner's new device"));
}

#[test]
fn a_table_from_before_consents_heals_and_then_lets_a_device_in() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut bob = Peer::new(&net, "bob");
    let (cid, gen) = community_with(&net, &mut alice, &mut [&mut bob]);
    assert_eq!(alice.c.debug_consents(&cid).len(), 2);
    alice.c.debug_clear_consents(&cid).unwrap();
    settle(&mut [&mut alice, &mut bob]);
    assert_eq!(alice.c.debug_consents(&cid).len(), 2, "both offered theirs again: {:?}", notices(&alice));
    let mut bob2 = pair(&net, &mut bob, &mut [&mut alice], "bob2");
    assert!(bob2.c.group_devices(&gen).contains(&bob2.c.my_device()), "{:?}", notices(&bob2));
}

#[test]
fn a_member_cannot_offer_consent_for_someone_else_or_edit_the_table() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut bob = Peer::new(&net, "bob");
    let mut carol = Peer::new(&net, "carol");
    let (cid, _) = community_with(&net, &mut alice, &mut [&mut bob, &mut carol]);
    let carols = alice.c.debug_consents(&cid).into_iter().find(|c| c.account == carol.acct()).unwrap();
    // Passed on by bob, carol's signature does not check out against bob as the sender.
    use commons_core::envelope::Envelope;
    bob.c.debug_send_envelope(&cid, Envelope::Consent { ts: carols.ts + 1, sig: carols.sig.clone(), relay_secret: None, relay_secret_want: false }).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert!(notices(&alice).iter().any(|n| n.contains("consent with a bad signature")), "{:?}", notices(&alice));
    assert_eq!(alice.c.debug_consents(&cid).len(), 3);
    let before = alice.c.epoch_fingerprint(&cid);
    assert!(bob.c.debug_clear_consents(&cid).is_err() || { settle(&mut [&mut alice, &mut bob, &mut carol]); alice.c.debug_consents(&cid).len() == 3 && alice.c.epoch_fingerprint(&cid) == before });
}

// ------------------------------------------------------------------ the relay's sign-up secret, across devices

fn gated_net() -> Net {
    Net::with_limits(commons_relay::Limits { invite_secret: Some("letmein".into()), ..Default::default() })
}
fn peer_with_secret(net: &Net, name: &str) -> Peer {
    let dir = tempfile::tempdir().unwrap();
    let (t, faults) = transport(&net.url());
    let mut c = Client::create_with_invite(dir.path(), "pw", &net.url(), name, "letmein", KdfParams::TEST, t).unwrap();
    impatient(&mut c);
    Peer { name: name.into(), c, dir, faults }
}

#[test]
fn a_paired_device_gets_the_secret_and_its_invite_signs_up_a_friend() {
    let net = gated_net();
    let mut alice = peer_with_secret(&net, "alice");
    let cid = alice.c.create_community("club").unwrap();
    assert_eq!(alice.c.st.relay_invite, "letmein");
    let mut alice2 = pair(&net, &mut alice, &mut [], "alice2");
    assert_eq!(alice2.c.st.relay_invite, "letmein", "pairing carried it, with no control group needed");
    settle(&mut [&mut alice, &mut alice2]);
    assert!(alice2.c.st.communities.contains_key(&cid), "{:?}", notices(&alice2));

    let made = alice2.c.create_invite_checked(&cid, Some(1), 3600).unwrap();
    assert!(!made.signup_needs_secret, "this device knows the secret");
    let link = commons_core::envelope::InviteLink::decode(&made.link).unwrap();
    assert_eq!(link.secret, "letmein");

    // Someone with no account at all can register with this link's secret, then ask to join.
    let dir = tempfile::tempdir().unwrap();
    let (t, faults) = transport(&net.url());
    let mut dave = Client::create_with_invite(dir.path(), "pw", &net.url(), "dave", &link.secret, KdfParams::TEST, t).unwrap();
    assert!(dave.st.registered);
    impatient(&mut dave);
    let mut dave = peer_from(dave, dir, faults, "dave");
    dave.c.join_with_link(&made.link).unwrap();
    settle(&mut [&mut alice, &mut alice2, &mut dave]);
    assert!(dave.c.st.communities.contains_key(&cid), "{:?}", notices(&dave));
}

#[test]
fn a_recovered_device_says_so_and_takes_the_secret_once() {
    let net = gated_net();
    let mut alice = peer_with_secret(&net, "alice");
    let mut bob = peer_with_secret(&net, "bob");
    let mut carol = peer_with_secret(&net, "carol");
    let (cid, _gen) = community_with(&net, &mut alice, &mut [&mut bob, &mut carol]);
    alice.c.set_role(&cid, &carol.acct(), commons_core::state::Role::Admin).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    let key = carol.c.recovery_key().unwrap();
    drop(carol); // the lost device stays offline, so only the recovered device remains

    let dir = tempfile::tempdir().unwrap();
    let (t, faults) = transport(&net.url());
    let recovered = Client::create_from_recovery(dir.path(), "newpw", &net.url(), "carol", &key, KdfParams::TEST, t).unwrap();
    assert!(recovered.st.relay_invite.is_empty(), "a recovery key does not carry the relay's sign-up secret");
    let mut carol2 = peer_from(recovered, dir, faults, "carol2");
    settle(&mut [&mut alice, &mut bob, &mut carol2]);
    assert!(carol2.c.st.communities.contains_key(&cid));

    let made = carol2.c.create_invite_checked(&cid, Some(1), 3600).unwrap();
    assert!(made.signup_needs_secret, "the relay asks for a secret this device does not have");
    assert!(commons_core::envelope::InviteLink::decode(&made.link).unwrap().secret.is_empty());

    assert!(carol2.c.set_relay_invite("not-it").is_err(), "the relay checks it, and this is not it");
    assert!(carol2.c.set_relay_invite("").is_err());
    carol2.c.set_relay_invite("letmein").unwrap();
    assert_eq!(carol2.c.st.relay_invite, "letmein");
    let made = carol2.c.create_invite_checked(&cid, Some(1), 3600).unwrap();
    assert!(!made.signup_needs_secret);
    assert_eq!(commons_core::envelope::InviteLink::decode(&made.link).unwrap().secret, "letmein");
}

#[test]
fn the_secret_spreads_between_one_accounts_devices_and_nobody_else_reads_it() {
    let net = gated_net();
    let mut alice = peer_with_secret(&net, "alice");
    let cid = alice.c.create_community("club").unwrap();
    // bob is a member of the same community, but his device does not know the secret (as if it had been
    // introduced before the relay was gated).
    let mut bob = peer_with_secret(&net, "bob");
    let link = alice.c.create_invite(&cid, None, 3600).unwrap();
    bob.c.join_with_link(&link).unwrap();
    bob.c.st.relay_invite.clear();
    settle(&mut [&mut alice, &mut bob]);

    // A second device of alice's account (a recovery) starts without it.
    let key = alice.c.recovery_key().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (t, faults) = transport(&net.url());
    let c = Client::create_from_recovery(dir.path(), "pw", &net.url(), "alice", &key, KdfParams::TEST, t).unwrap();
    assert!(c.st.relay_invite.is_empty());
    let mut alice2 = peer_from(c, dir, faults, "alice2");
    settle(&mut [&mut alice, &mut alice2, &mut bob]);

    assert_eq!(alice2.c.st.relay_invite, "letmein", "the account's other device passed it on: {:?}", notices(&alice2));
    assert!(bob.c.st.relay_invite.is_empty(), "another account cannot open the sealed secret");
    // Nothing anywhere looks like an unknown kind of message: old clients parse these as plain consents.
    for p in [&alice, &alice2, &bob] {
        assert!(!notices(p).iter().any(|n| n.starts_with("newer:")), "{} saw an unknown kind: {:?}", p.name, notices(p));
    }
}
