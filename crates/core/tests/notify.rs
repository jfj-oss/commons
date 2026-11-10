//! Notifications from real clients on a real relay: what the scanner sees as a new message, who it names, and what a
//! notification says. (The rules, hand-over and coalescing are unit-tested in src/notify.rs.)
mod common;
use common::*;
use commons_core::envelope::ROOM_VOICE;
use commons_core::notify::*;
use std::time::{Duration, Instant};

#[test]
fn new_messages_become_notifications_with_the_right_words() {
    let net = Net::new();
    let (mut alice, mut bob, mut carol) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"), Peer::new(&net, "carol"));
    let (cid, gen) = community_with(&net, &mut alice, &mut [&mut bob, &mut carol]);
    let lounge = alice.c.create_channel_kind(&cid, "lounge", ROOM_VOICE).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    let mine = alice.c.send_text(&gen, "my own question", None).unwrap();
    bob.c.send_text(&gen, "old news from before the program started", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);

    let mut sc = Scanner::default();
    assert!(scan_client(&alice.c, &mut sc).is_empty(), "starting up announces nothing that was already waiting");
    assert!(scan_client(&alice.c, &mut sc).is_empty());

    // a plain message, an @mention, a reply to my message, and the sender's name in the room's words
    bob.c.send_text(&gen, "the secret plan is at noon", None).unwrap();
    carol.c.send_text(&gen, "hey @alice, are you there?", None).unwrap();
    bob.c.send_text(&gen, "answering you", Some(mine.clone())).unwrap();
    bob.c.send_text(&gen, "mail me@alice.example is not a mention", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    let evs = scan_client(&alice.c, &mut sc);
    assert_eq!(evs.len(), 4, "{evs:?}");
    let why: Vec<(String, Why)> = evs.iter().map(|e| (e.who.clone(), e.why)).collect();
    assert_eq!(why.iter().filter(|(_, w)| *w == Why::Mention).count(), 1);
    assert!(evs.iter().any(|e| e.who == "carol" && e.why == Why::Mention));
    assert!(evs.iter().any(|e| e.who == "bob" && e.why == Why::Reply && e.text == "answering you"));
    assert!(evs.iter().filter(|e| e.why == Why::Plain).count() == 2);
    assert!(evs.iter().all(|e| e.gid == gen && e.room == "general"));
    assert!(scan_client(&alice.c, &mut sc).is_empty(), "each message is announced once");

    // through the hub: no page, so the background part shows it, and with text off nothing of the message is in it
    let hub = Hub::new(None);
    let t = Instant::now();
    let notes = hub.step(evs.clone(), t, 1_000_000);
    assert_eq!(notes.len(), 1, "one note for the room");
    assert!(notes[0].body.contains("general") && !notes[0].body.contains("secret") && !notes[0].title.contains("secret"), "{:?}", notes[0]);
    hub.set_show_text(true);
    let notes = hub.step(evs, t + Duration::from_secs(10), 1_000_000);
    assert!(notes[0].body.contains("(+3 more)") || notes[0].title.contains("mentioned"), "{:?}", notes[0]);

    // my own messages, a voice room, a deleted message, and a room I have read are not announced
    alice.c.send_text(&gen, "i said this", None).unwrap();
    let doomed = bob.c.send_text(&gen, "oops", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    bob.c.delete_message(&gen, &doomed).unwrap();
    bob.c.send_text(&lounge, "words in a voice room", None).ok();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    let evs = scan_client(&alice.c, &mut sc);
    assert!(evs.iter().all(|e| e.text != "i said this" && e.gid != lounge && e.text != "words in a voice room"), "{evs:?}");
    alice.c.mark_read(&gen);
    assert!(scan_client(&alice.c, &mut sc).is_empty());
}
