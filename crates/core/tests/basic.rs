mod common;
use common::*;
use commons_core::state::{MsgBody, Role};

#[test]
fn community_chat_end_to_end() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut bob = Peer::new(&net, "bob");
    let mut carol = Peer::new(&net, "carol");
    let (cid, gen) = community_with(&net, &mut alice, &mut [&mut bob, &mut carol]);

    // everyone is in the community and the channel, same epoch keys
    for p in [&alice, &bob, &carol] {
        assert_eq!(p.c.group_accounts(&cid).len(), 3, "{} sees 3 members", p.name);
        assert_eq!(p.c.group_accounts(&gen).len(), 3);
        assert_eq!(p.c.st.communities[&cid].name, "club");
    }
    assert_eq!(alice.c.epoch_fingerprint(&gen), bob.c.epoch_fingerprint(&gen));
    assert_eq!(alice.c.epoch_fingerprint(&gen), carol.c.epoch_fingerprint(&gen));
    assert_eq!(alice.c.epoch_fingerprint(&cid), carol.c.epoch_fingerprint(&cid));
    assert_eq!(bob.c.display_name(&alice.acct()), "alice");

    // messages, reply, edit, reaction, delete
    let m1 = alice.c.send_text(&gen, "hello everyone", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert!(has_text(&bob, &gen, "hello everyone") && has_text(&carol, &gen, "hello everyone"));
    let m2 = bob.c.send_text(&gen, "hi alice", Some(m1.clone())).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    let got = alice.c.messages(&gen).into_iter().find(|m| m.id == m2).unwrap();
    assert_eq!(got.reply.as_deref(), Some(m1.as_str()));
    bob.c.edit_message(&gen, &m2, "hi alice (edited)").unwrap();
    carol.c.react(&gen, &m2, "👍", true).unwrap();
    alice.c.react(&gen, &m2, "👍", true).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    let got = alice.c.messages(&gen).into_iter().find(|m| m.id == m2).unwrap();
    assert!(got.edited && matches!(&got.body, MsgBody::Text(t) if t == "hi alice (edited)"));
    assert_eq!(got.reactions["👍"].len(), 2);
    // a member cannot delete someone else's message, an admin/owner can
    assert!(carol.c.delete_message(&gen, &m2).is_err());
    alice.c.delete_message(&gen, &m2).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert!(!has_text(&carol, &gen, "hi alice (edited)"));

    // unread counts and search
    carol.c.mark_read(&gen);
    alice.c.send_text(&gen, "the quick brown fox", None).unwrap();
    alice.c.send_text(&gen, "another one", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert_eq!(carol.c.unread(&gen), 2);
    assert_eq!(carol.c.search("brown").len(), 1);
    assert_eq!(carol.c.search("zzzz").len(), 0);

    // file: encrypted blob on the relay
    let data = (0..70000u32).map(|i| (i % 251) as u8).collect::<Vec<_>>();
    let fid = alice.c.send_file(&gen, "pic.bin", "application/octet-stream", &data, "a file").unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    let (name, _mime, bytes) = bob.c.fetch_file(&gen, &fid).unwrap();
    assert_eq!(name, "pic.bin"); assert_eq!(bytes, data);

    // more channels
    let ch2 = alice.c.create_channel(&cid, "random stuff").unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert_eq!(carol.c.group_accounts(&ch2).len(), 3);
    assert_eq!(carol.c.channels_of(&cid).len(), 2);
    assert!(carol.c.create_channel(&cid, "nope").is_err(), "members cannot create channels");
    bob.c.send_text(&ch2, "in the new channel", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert!(has_text(&alice, &ch2, "in the new channel"));

    // roles
    assert!(bob.c.set_role(&cid, &carol.acct(), Role::Admin).is_err());
    alice.c.set_role(&cid, &bob.acct(), Role::Admin).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert_eq!(carol.c.st.communities[&cid].role_of(&bob.acct()), Some(Role::Admin));
    bob.c.create_channel(&cid, "bobs").unwrap(); // admin may now
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert_eq!(alice.c.channels_of(&cid).len(), 3);

    // restart keeps everything (history is stored encrypted)
    let before = texts(&carol, &gen);
    carol.reopen(&net);
    assert_eq!(texts(&carol, &gen), before);
    let raw = std::fs::read(carol.dir.path().join("state.cmns")).unwrap();
    assert!(!raw.windows(5).any(|w| w == b"brown"), "state file must not contain plaintext");
    assert!(commons_core::client::Client::open(carol.dir.path(), "wrong", commons_core::transport::HttpTransport::new(&net.url()).into_arc()).is_err());
}
