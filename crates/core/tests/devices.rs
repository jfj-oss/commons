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
