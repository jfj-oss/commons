mod common;
use common::*;
use commons_core::envelope::Envelope;

/// A tiny fake "image": the core treats a picture as opaque bytes, as it does for a file. The marker is what the
/// relay must never see.
fn picture(marker: &str) -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
    for _ in 0..64 { v.extend_from_slice(marker.as_bytes()); }
    v
}

#[test]
fn a_picture_reaches_everyone_and_changes_and_goes_away() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut bob = Peer::new(&net, "bob");
    let mut carol = Peer::new(&net, "carol");
    let (cid, gen) = community_with(&net, &mut alice, &mut [&mut bob, &mut carol]);
    let a = alice.acct();

    // Alice sets a picture; one account-wide message, carried by the profile.
    let first = picture("ALICE-FIRST-PICTURE");
    let r1 = alice.c.set_avatar("image/webp", &first).unwrap();
    assert!(r1.size as usize <= 256 * 1024);
    // It is visible at once on her own device, before any round trip.
    assert_eq!(alice.c.avatar_of(&a).as_ref(), Some(&r1));
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    for p in [&mut bob, &mut carol] {
        assert_eq!(p.c.avatar_of(&a).as_ref(), Some(&r1), "{} has Alice's picture", p.name);
        let (mime, bytes) = p.c.fetch_avatar(&a).unwrap();
        assert_eq!(mime, "image/webp");
        assert_eq!(bytes, first, "{} got the exact picture", p.name);
    }
    // The relay only ever held the ciphertext.
    assert!(!contains(&relay_bytes(&net), "ALICE-FIRST-PICTURE"));

    // A name change re-broadcasts the profile, and the picture travels with it.
    alice.c.set_name("alice renamed").unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert_eq!(bob.c.display_name(&a), "alice renamed");
    assert_eq!(bob.c.avatar_of(&a).as_ref(), Some(&r1), "a name change keeps the picture");

    // A change replaces it for everyone; the old bytes are no longer what anyone asks for.
    let second = picture("ALICE-SECOND-PICTURE");
    let r2 = alice.c.set_avatar("image/jpeg", &second).unwrap();
    assert_ne!(r1.blob, r2.blob);
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert_eq!(carol.c.avatar_of(&a).as_ref(), Some(&r2));
    assert_eq!(carol.c.fetch_avatar(&a).unwrap().1, second);
    assert!(!contains(&relay_bytes(&net), "ALICE-SECOND-PICTURE"));

    // One picture per account, not per community: a second community sees the same one, and its own members change
    // nothing about that.
    let cid2 = alice.c.create_community("other club").unwrap();
    let link = alice.c.create_invite(&cid2, None, 3600).unwrap();
    bob.c.join_with_link(&link).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert_eq!(bob.c.avatar_of(&a).as_ref(), Some(&r2), "same picture in the second community");
    assert!(bob.c.st.communities.contains_key(&cid));

    // Removal goes back to the seal everywhere.
    alice.c.avatar_clear().unwrap();
    assert_eq!(alice.c.avatar_of(&a), None);
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    for p in [&alice, &bob, &carol] {
        assert_eq!(p.c.avatar_of(&a), None, "{} dropped the removed picture", p.name);
    }
    // The channel still works and Bob's own (absent) picture is untouched by any of this.
    bob.c.send_text(&gen, "still here", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert!(has_text(&alice, &gen, "still here"));
    assert_eq!(bob.c.avatar_of(&bob.acct()), None);
}

#[test]
fn a_late_member_and_a_restart_still_have_the_picture() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut bob = Peer::new(&net, "bob");
    community_with(&net, &mut alice, &mut [&mut bob]);
    let a = alice.acct();
    let bytes = picture("ALICE-PICTURE-FOR-LATE-JOINER");
    let r = alice.c.set_avatar("image/webp", &bytes).unwrap();
    settle(&mut [&mut alice, &mut bob]);

    // Dave joins later: a member change marks the profile dirty, and the next broadcast carries the picture.
    let mut dave = Peer::new(&net, "dave");
    let cid = alice.c.st.communities.keys().next().unwrap().clone();
    let link = alice.c.create_invite(&cid, None, 3600).unwrap();
    dave.c.join_with_link(&link).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut dave]);
    assert_eq!(dave.c.avatar_of(&a).as_ref(), Some(&r), "the newcomer sees the picture");
    assert_eq!(dave.c.fetch_avatar(&a).unwrap().1, bytes);

    // It survives a restart from the local cache, with the relay unreachable. (Bob fetches it once while online.)
    assert_eq!(bob.c.fetch_avatar(&a).unwrap().1, bytes);
    bob.reopen(&net);
    bob.faults.lock().unwrap().offline = true;
    assert_eq!(bob.c.avatar_of(&a).as_ref(), Some(&r));
    assert_eq!(bob.c.fetch_avatar(&a).unwrap().1, bytes, "cached picture works offline");
}

#[test]
fn a_profile_from_an_older_app_does_not_wipe_the_picture() {
    // The 0.2.x wire form has no picture fields at all. A newer client must read it as a name-only update, not as a
    // removal: this is the same message shape older apps send, parsed by the current parser.
    let old = br#"{"t":"Profile","name":"alice from 0.2.3","ts":4102444800}"#;
    let parsed = Envelope::parse(old).unwrap();
    assert!(matches!(parsed, Envelope::Profile { ref name, avatar: None, avatar_cleared: false, .. } if name == "alice from 0.2.3"));
    // A picture reference, on the other hand, comes through as an ignored extra field for old apps and is understood
    // here (the wire compatibility itself is proved in envelope.rs's tests).
    let with = br#"{"t":"Profile","name":"alice","ts":4102444800,"avatar":{"blob":"00112233445566778899aabbccddeeff","key":"000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f","nonce":"000102030405060708090a0b","mime":"image/webp","size":1234,"shape":"circle"}}"#;
    let parsed = Envelope::parse(with).unwrap();
    assert!(matches!(parsed, Envelope::Profile { avatar: Some(ref a), avatar_cleared: false, .. } if a.mime == "image/webp"));
}
