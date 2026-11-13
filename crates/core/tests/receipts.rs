//! Read receipts (receipts.rs): one more encrypted envelope kind, `Read`, which an older app skips as "newer version". Sent
//! only when a room was looked at, only forward and not too often; kept per member; and all of it off for whoever turns it off.
mod common;
use common::*;
use commons_core::envelope::*;

fn id(n: u8) -> String { format!("{:032x}", n) }
fn reads_waiting(p: &Peer, gid: &str) -> Vec<String> {
    p.c.st.outbox.iter().filter(|i| i.gid == gid).filter_map(|i| match &i.env { Envelope::Read { upto } => Some(upto.clone()), _ => None }).collect()
}
fn read_by(p: &Peer, gid: &str, who: &Peer) -> Option<String> { p.c.st.chans.get(gid).and_then(|l| l.read_by.get(&who.acct()).cloned()) }
/// What the page is given: message id -> names, for the whole room.
fn marks(p: &Peer, gid: &str) -> Vec<(String, Vec<String>)> {
    let log = p.c.message_log(gid);
    let mut v: Vec<(String, Vec<String>)> = p.c.read_marks(gid, &log).into_iter().map(|(k, n)| (k.to_string(), n)).collect();
    v.sort();
    v
}
fn now_ms() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64 }
const HOUR: u64 = 3_600_000;
/// Receipts wait an hour for each other, so only the times a test gives decide what is sent.
fn slow(p: &mut Peer) { p.c.read_gap_ms = HOUR; }

#[test]
fn read_envelope_parses_and_old_apps_skip_it() {
    let e = Envelope::Read { upto: id(7) };
    assert_eq!(Envelope::parse(&e.to_bytes()).unwrap(), e);
    assert_eq!(String::from_utf8(e.to_bytes()).unwrap(), format!("{{\"t\":\"Read\",\"upto\":\"{}\"}}", id(7)), "a message id and nothing else");
    assert!(Envelope::parse(&Envelope::Read { upto: "42".into() }.to_bytes()).is_err(), "a position in a list is not a message id");
    assert!(Envelope::parse(&Envelope::Read { upto: String::new() }.to_bytes()).is_err());
    assert!(Envelope::parse(br#"{"t":"Read"}"#).is_err());
    // What an app from before receipts does with it: it is `Unknown` there, which that app skips with a "newer version" note.
    #[derive(serde::Deserialize, Debug, PartialEq)]
    #[serde(tag = "t")]
    enum Old { Typing, Text { text: String }, #[serde(other)] Unknown }
    assert_eq!(serde_json::from_slice::<Old>(&e.to_bytes()).unwrap(), Old::Unknown);
    let _ = Old::Text { text: String::new() };
    let _ = Old::Typing;
}

#[test]
fn each_member_has_their_own_place_and_it_is_kept() {
    let net = Net::new();
    let (mut alice, mut bob, mut carol) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"), Peer::new(&net, "carol"));
    let (_cid, gen) = community_with(&net, &mut alice, &mut [&mut bob, &mut carol]);
    for p in [&mut alice, &mut bob, &mut carol] { slow(p); }
    assert!(alice.c.read_receipts_on(), "on for a new account");
    let m1 = alice.c.send_text(&gen, "one", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    // bob looks at the room; carol does not
    bob.c.mark_read(&gen); bob.c.note_read(&gen);
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert_eq!(read_by(&alice, &gen, &bob), Some(m1.clone()));
    assert_eq!(read_by(&carol, &gen, &bob), Some(m1.clone()), "everyone in the room is told");
    assert_eq!(read_by(&alice, &gen, &carol), None, "carol has not looked");
    assert_eq!(marks(&alice, &gen), vec![(m1.clone(), vec!["bob".to_string()])]);
    assert_eq!(marks(&carol, &gen), vec![(m1.clone(), vec!["bob".to_string()])]);
    assert_eq!(marks(&bob, &gen), vec![], "nobody is shown their own place");
    // a second message: carol reads up to it, bob stays where he was
    let m2 = alice.c.send_text(&gen, "two", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    carol.c.mark_read(&gen); carol.c.note_read(&gen);
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    let mut two = vec![(m1.clone(), vec!["bob".to_string()]), (m2.clone(), vec!["carol".to_string()])];
    two.sort();
    assert_eq!(marks(&alice, &gen), two);
    assert_eq!(marks(&bob, &gen), vec![(m2.clone(), vec!["carol".to_string()])]);
    // bob catches up: both names on the one row, in order
    bob.c.note_read_at(&gen, now_ms() + 2 * HOUR);
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert_eq!(marks(&alice, &gen), vec![(m2.clone(), vec!["bob".to_string(), "carol".to_string()])]);
    // the row is deleted: the place moves to the row before it rather than vanishing
    alice.c.delete_message(&gen, &m2).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert_eq!(marks(&alice, &gen), vec![(m1.clone(), vec!["bob".to_string(), "carol".to_string()])]);
    // a page that only has later rows shows no mark for a place above them
    let m3 = alice.c.send_text(&gen, "three", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    let log = alice.c.message_log(&gen);
    let tail: Vec<_> = log.iter().copied().filter(|m| m.id == m3).collect();
    assert!(alice.c.read_marks(&gen, &tail).is_empty());
    // saved with the history: still there after a restart
    alice.reopen(&net);
    assert_eq!(read_by(&alice, &gen, &bob), Some(m2.clone()));
    assert_eq!(marks(&alice, &gen), vec![(m1.clone(), vec!["bob".to_string(), "carol".to_string()])]);
    // nothing of it is readable on the relay
    assert!(!contains(&relay_bytes(&net), "\"Read\"") && !contains(&relay_bytes(&net), &m1));
}

#[test]
fn receipts_only_go_forward_and_not_too_often() {
    let net = Net::new();
    let (mut alice, mut bob) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (_cid, gen) = community_with(&net, &mut alice, &mut [&mut bob]);
    slow(&mut alice); slow(&mut bob);
    let t0 = now_ms();
    // nothing to read, nothing to say; and a room that was not looked at says nothing however much arrives
    bob.c.note_read_at(&gen, t0);
    assert!(reads_waiting(&bob, &gen).is_empty() && bob.c.st.chans.get(&gen).is_none_or(|l| l.read_want.is_none()));
    let m1 = alice.c.send_text(&gen, "one", None).unwrap();
    let m2 = alice.c.send_text(&gen, "two", None).unwrap();
    settle(&mut [&mut alice, &mut bob]);
    assert_eq!(read_by(&alice, &gen, &bob), None);
    // the first look: one receipt, at once, for the newest message
    bob.c.note_read_at(&gen, t0);
    assert_eq!(reads_waiting(&bob, &gen), vec![m2.clone()]);
    // looking again without anything new: nothing more
    bob.c.note_read_at(&gen, t0 + 1);
    bob.c.note_read_at(&gen, t0 + 2 * HOUR);
    assert_eq!(reads_waiting(&bob, &gen), vec![m2.clone()]);
    settle(&mut [&mut alice, &mut bob]);
    assert_eq!(read_by(&alice, &gen, &bob), Some(m2.clone()));
    // bob's own message moves nothing: a receipt is for what somebody else wrote
    bob.c.send_text(&gen, "mine", None).unwrap();
    settle(&mut [&mut alice, &mut bob]);
    bob.c.note_read_at(&gen, t0 + 2 * HOUR);
    assert!(reads_waiting(&bob, &gen).is_empty());
    // inside the pause: the next one waits, and only the newest one waiting is ever sent
    let m3 = alice.c.send_text(&gen, "three", None).unwrap();
    settle(&mut [&mut alice, &mut bob]);
    bob.c.note_read_at(&gen, t0 + 1000);
    assert!(reads_waiting(&bob, &gen).is_empty(), "too soon after the last one");
    assert_eq!(bob.c.st.chans[&gen].read_want, Some(m3.clone()));
    let m4 = alice.c.send_text(&gen, "four", None).unwrap();
    settle(&mut [&mut alice, &mut bob]);
    assert_eq!(read_by(&alice, &gen, &bob), Some(m2.clone()), "rounds in between do not send it early");
    bob.c.note_read_at(&gen, t0 + 2000);
    assert!(reads_waiting(&bob, &gen).is_empty());
    assert_eq!(bob.c.st.chans[&gen].read_want, Some(m4.clone()));
    bob.c.flush_reads_at(t0 + HOUR - 1);
    assert!(reads_waiting(&bob, &gen).is_empty(), "still inside the pause");
    bob.c.flush_reads_at(t0 + HOUR);
    assert_eq!(reads_waiting(&bob, &gen), vec![m4.clone()], "one receipt for the two messages");
    assert_eq!(bob.c.st.chans[&gen].read_want, None);
    settle(&mut [&mut alice, &mut bob]);
    assert_eq!(read_by(&alice, &gen, &bob), Some(m4.clone()));
    // the receiver keeps it forward too: an older place, or a message it has never seen, changes nothing
    bob.c.debug_send_envelope(&gen, Envelope::Read { upto: m1.clone() }).unwrap();
    bob.c.debug_send_envelope(&gen, Envelope::Read { upto: id(99) }).unwrap();
    settle(&mut [&mut alice, &mut bob]);
    assert_eq!(read_by(&alice, &gen, &bob), Some(m4.clone()));
    // receipts are not messages: no row, no unread
    assert_eq!(texts(&alice, &gen), vec!["one", "two", "mine", "three", "four"]);
    assert!(!notices(&alice).iter().any(|n| n.starts_with("newer") || n.starts_with("reject")), "{:?}", notices(&alice));
    alice.c.mark_read(&gen);
    bob.c.debug_send_envelope(&gen, Envelope::Read { upto: m4.clone() }).unwrap();
    settle(&mut [&mut alice, &mut bob]);
    assert_eq!(alice.c.unread(&gen), 0);
    // a receipt that could not leave is replaced by the next, not followed by it
    let m5 = alice.c.send_text(&gen, "five", None).unwrap();
    settle(&mut [&mut alice, &mut bob]);
    bob.faults.lock().unwrap().offline = true;
    bob.c.st.outbox.push(commons_core::state::OutItem { id: id(50), gid: gen.clone(), env: Envelope::Read { upto: m4.clone() }, cipher: None, tries: 0 });
    bob.c.note_read_at(&gen, t0 + 3 * HOUR);
    assert_eq!(reads_waiting(&bob, &gen), vec![m5.clone()]);
}

#[test]
fn turned_off_it_sends_nothing_and_shows_nothing() {
    let net = Net::new();
    let (mut alice, mut bob, mut carol) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"), Peer::new(&net, "carol"));
    let (_cid, gen) = community_with(&net, &mut alice, &mut [&mut bob, &mut carol]);
    for p in [&mut alice, &mut bob, &mut carol] { slow(p); }
    bob.c.set_read_receipts(false);
    bob.c.save().unwrap();
    let m1 = alice.c.send_text(&gen, "one", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    let before = relay_entries(&net, &gen).len();
    // bob reads: nothing leaves his device
    bob.c.mark_read(&gen); bob.c.note_read(&gen);
    assert!(reads_waiting(&bob, &gen).is_empty() && bob.c.st.chans[&gen].read_want.is_none());
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert_eq!(relay_entries(&net, &gen).len(), before, "not one more entry on the relay");
    assert_eq!(read_by(&alice, &gen, &bob), None);
    // carol reads: alice sees it, bob is shown nothing
    carol.c.mark_read(&gen); carol.c.note_read(&gen);
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert_eq!(relay_entries(&net, &gen).len(), before + 1);
    assert_eq!(marks(&alice, &gen), vec![(m1.clone(), vec!["carol".to_string()])]);
    assert_eq!(marks(&bob, &gen), vec![], "whoever does not send does not see");
    // the choice is saved
    bob.reopen(&net); slow(&mut bob);
    assert!(!bob.c.read_receipts_on());
    assert_eq!(marks(&bob, &gen), vec![]);
    // on again: he sees the others' and sends his own from the next look on
    bob.c.set_read_receipts(true);
    assert_eq!(marks(&bob, &gen), vec![(m1.clone(), vec!["carol".to_string()])]);
    bob.c.note_read(&gen);
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert_eq!(marks(&alice, &gen), vec![(m1.clone(), vec!["bob".to_string(), "carol".to_string()])]);
    // turning it off takes back what has not left yet: one waiting for its pause, and one in the outbox
    let m2 = alice.c.send_text(&gen, "two", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    let t = now_ms();
    bob.c.note_read_at(&gen, t);
    assert_eq!(bob.c.st.chans[&gen].read_want, Some(m2.clone()), "waiting: the last one went a moment ago");
    carol.c.note_read_at(&gen, t + 2 * HOUR);
    assert_eq!(reads_waiting(&carol, &gen), vec![m2.clone()]);
    bob.c.set_read_receipts(false);
    carol.c.set_read_receipts(false);
    assert!(bob.c.st.chans[&gen].read_want.is_none() && reads_waiting(&carol, &gen).is_empty());
    bob.c.flush_reads_at(t + 2 * HOUR);
    let before = relay_entries(&net, &gen).len();
    settle(&mut [&mut alice, &mut bob, &mut carol]);
    assert_eq!(relay_entries(&net, &gen).len(), before);
    assert_eq!(marks(&alice, &gen), vec![(m1.clone(), vec!["bob".to_string(), "carol".to_string()])], "their places stay where they last said");
}

#[test]
fn marking_read_alone_never_sends_which_is_all_a_bot_does() {
    let net = Net::new();
    let (mut alice, mut bot) = (Peer::new(&net, "alice"), Peer::new(&net, "oracle"));
    let (_cid, gen) = community_with(&net, &mut alice, &mut [&mut bot]);
    alice.c.send_text(&gen, "one", None).unwrap();
    settle(&mut [&mut alice, &mut bot]);
    let before = relay_entries(&net, &gen).len();
    bot.c.mark_read(&gen);
    bot.c.save().unwrap();
    settle(&mut [&mut alice, &mut bot]);
    assert_eq!(bot.c.unread(&gen), 0);
    assert_eq!(relay_entries(&net, &gen).len(), before);
    assert_eq!(read_by(&alice, &gen, &bot), None);
    // and with a bot in the room that has them off, people's receipts work as ever
    bot.c.set_read_receipts(false);
    let m = bot.c.send_text(&gen, "answer", None).unwrap();
    settle(&mut [&mut alice, &mut bot]);
    alice.c.mark_read(&gen); alice.c.note_read(&gen);
    settle(&mut [&mut alice, &mut bot]);
    assert_eq!(read_by(&bot, &gen, &alice), Some(m));
    assert_eq!(marks(&bot, &gen), vec![]);
}

#[test]
fn the_switch_is_in_the_snapshot_and_mark_read_is_what_sends() {
    let net = Net::new();
    let dir = tempfile::tempdir().unwrap();
    let app = commons_core::api::App::with_kdf(dir.path().to_path_buf(), &net.url(), commons_core::vault::KdfParams::TEST);
    app.call("create_account", serde_json::json!({"name": "A", "passphrase": "correct horse battery", "relay": net.url()})).unwrap();
    assert_eq!(app.call("snapshot", serde_json::json!({})).unwrap()["read_receipts"], true);
    app.call("set_read_receipts", serde_json::json!({"on": false})).unwrap();
    assert_eq!(app.call("snapshot", serde_json::json!({})).unwrap()["read_receipts"], false);
    app.call("set_read_receipts", serde_json::json!({"on": true})).unwrap();
    assert_eq!(app.call("snapshot", serde_json::json!({})).unwrap()["read_receipts"], true);
}
