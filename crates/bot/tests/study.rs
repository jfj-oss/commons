//! Study reminders end to end: a fake planner (a small HTTP server that behaves like the real one: the reminders-only token, a
//! "sent" list, stale and muted reminders left out) and a throwaway local relay with real accounts.  The real planner, the real
//! bot and any real room are never touched.
mod common;
use common::*;
use commons_bot::bot::SharedClient;
use commons_bot::study::{HttpStudy, Outcome, Runner};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

const TOKEN: &str = "reminders-only-test-token";

/// One reminder the planner knows: id, text, and what makes it skipped (the planner does this filtering, not the bot).
#[derive(Clone)]
struct R { id: &'static str, text: &'static str, stale: bool, muted: bool }

struct Planner { list: Mutex<Vec<R>>, sent: Mutex<BTreeSet<String>>, down: Mutex<bool>, bad_calls: Mutex<usize> }
impl Planner {
    fn new(list: Vec<R>) -> Arc<Planner> { Arc::new(Planner { list: Mutex::new(list), sent: Default::default(), down: Mutex::new(false), bad_calls: Mutex::new(0) }) }
    fn serve(self: &Arc<Planner>) -> FakeHttp {
        let p = self.clone();
        FakeHttp::start(move |r| {
            if r.header("x-homebase-token") != Some(TOKEN) { *p.bad_calls.lock().unwrap() += 1; return (401, json!({"ok": false}).to_string()); }
            if *p.down.lock().unwrap() { return (503, "{}".into()); }
            match (r.method.as_str(), r.path.split('?').next().unwrap()) {
                ("GET", "/api/reminders/due") => {
                    let sent = p.sent.lock().unwrap();
                    let due: Vec<Value> = p.list.lock().unwrap().iter().filter(|x| !x.stale && !x.muted && !sent.contains(x.id))
                        .map(|x| json!({"id": x.id, "text": x.text, "course": "HIST 398", "at": "2026-11-16T08:00"})).collect();
                    (200, json!({"ok": true, "now": "2026-11-16T18:31", "reminders": due}).to_string())
                }
                ("POST", "/api/reminders/sent") => {
                    let ids = r.json()["ids"].as_array().cloned().unwrap_or_default();
                    p.sent.lock().unwrap().extend(ids.iter().filter_map(|i| i.as_str().map(String::from)));
                    (200, json!({"ok": true}).to_string())
                }
                _ => (404, "{}".into()),
            }
        })
    }
}

fn token_file(dir: &std::path::Path, token: &str) -> std::path::PathBuf {
    let f = dir.join("study-reminders-token");
    std::fs::write(&f, format!("{token}\n")).unwrap();
    f
}

fn oracle_posts(student: &Peer, gid: &str, oracle: &str) -> Vec<String> {
    student.c.messages(gid).into_iter().filter(|m| m.sender == oracle && !m.deleted)
        .filter_map(|m| match m.body { commons_core::state::MsgBody::Text(t) => Some(t), _ => None }).collect()
}

fn pump(student: &mut Peer, oracle: &mut Peer) { settle(&mut [student, oracle]); }

#[test]
fn due_reminders_are_posted_once_confirmed_and_not_repeated_after_a_restart() {
    let net = Net::new();
    let mut student = Peer::new(&net, "student");
    let mut oracle = Peer::new(&net, "Oracle");
    let (_cid, gen) = community_with(&net, &mut student, &mut [&mut oracle]);
    let oracle_id = oracle.acct();
    let planner = Planner::new(vec![
        R { id: "7:2", text: "HIST 398: In-Class Essay Exam 2 (25%) is due in 2 days, Wed Nov 18 at 1:15 pm.", stale: false, muted: false },
        R { id: "12:0", text: "CLAS 267: Quiz 7 (1.5%) closes today.", stale: false, muted: false },
        R { id: "3:0", text: "OLD: was due last week.", stale: true, muted: false },
        R { id: "9:2", text: "MUTED COURSE: paper.", stale: false, muted: true },
    ]);
    let srv = planner.serve();
    let tdir = tempfile::tempdir().unwrap();
    let tf = token_file(tdir.path(), TOKEN);

    let mut runner = Runner::new(Box::new(HttpStudy::new(&srv.url(), &tf)), &gen);
    let shared = Mutex::new(oracle.c);
    assert_eq!(runner.tick(&mut SharedClient(&shared)), Outcome::Posted);
    // a second minute: nothing is due any more
    assert_eq!(runner.tick(&mut SharedClient(&shared)), Outcome::Idle);
    oracle.c = shared.into_inner().unwrap();
    pump(&mut student, &mut oracle);
    let posts = oracle_posts(&student, &gen, &oracle_id);
    assert_eq!(posts.len(), 1, "one message for both: {posts:?}");
    assert!(posts[0].starts_with("Reminders:\n- HIST 398"), "{posts:?}");
    assert!(posts[0].contains("CLAS 267") && !posts[0].contains("OLD") && !posts[0].contains("MUTED"), "stale and muted are not posted: {posts:?}");
    assert_eq!(*planner.sent.lock().unwrap(), BTreeSet::from(["12:0".to_string(), "7:2".to_string()]), "recorded as sent");
    assert_eq!(*planner.bad_calls.lock().unwrap(), 0);

    // the bot restarts: a new runner (empty `since`) asks again and the planner offers nothing, so nothing is reposted
    let shared = Mutex::new(oracle.c);
    let mut again = Runner::new(Box::new(HttpStudy::new(&srv.url(), &tf)), &gen);
    assert_eq!(again.tick(&mut SharedClient(&shared)), Outcome::Idle);
    oracle.c = shared.into_inner().unwrap();
    pump(&mut student, &mut oracle);
    assert_eq!(oracle_posts(&student, &gen, &oracle_id).len(), 1);
}

#[test]
fn a_planner_that_is_down_or_a_wrong_token_posts_nothing_and_a_late_planner_is_caught_up() {
    let net = Net::new();
    let mut student = Peer::new(&net, "student");
    let mut oracle = Peer::new(&net, "Oracle");
    let (_cid, gen) = community_with(&net, &mut student, &mut [&mut oracle]);
    let oracle_id = oracle.acct();
    let planner = Planner::new(vec![R { id: "7:2", text: "HIST 398: exam in 2 days.", stale: false, muted: false }]);
    let srv = planner.serve();
    let tdir = tempfile::tempdir().unwrap();
    let good = token_file(tdir.path(), TOKEN);
    let shared = Mutex::new(oracle.c);

    // wrong token: refused, nothing posted, nothing recorded, and the error text never contains the token
    let wrong = tdir.path().join("wrong");
    std::fs::write(&wrong, "not-the-token\n").unwrap();
    let mut r = Runner::new(Box::new(HttpStudy::new(&srv.url(), &wrong)), &gen);
    match r.tick(&mut SharedClient(&shared)) { Outcome::Failed(e) => assert!(!e.contains("not-the-token") && !e.contains(TOKEN)), o => panic!("{o:?}") }
    // no token file at all
    let mut r = Runner::new(Box::new(HttpStudy::new(&srv.url(), &tdir.path().join("missing"))), &gen);
    assert!(matches!(r.tick(&mut SharedClient(&shared)), Outcome::Failed(_)));
    assert!(planner.sent.lock().unwrap().is_empty());

    // planner down, then back: the reminder goes out when it is back
    let mut r = Runner::new(Box::new(HttpStudy::new(&srv.url(), &good)), &gen);
    *planner.down.lock().unwrap() = true;
    assert!(matches!(r.tick(&mut SharedClient(&shared)), Outcome::Failed(_)));
    *planner.down.lock().unwrap() = false;
    assert_eq!(r.tick(&mut SharedClient(&shared)), Outcome::Posted);
    oracle.c = shared.into_inner().unwrap();
    pump(&mut student, &mut oracle);
    assert_eq!(oracle_posts(&student, &gen, &oracle_id), ["Reminder: HIST 398: exam in 2 days."]);
}

#[test]
fn a_room_the_owner_made_quiet_gets_nothing_and_nothing_is_recorded() {
    let net = Net::new();
    let mut student = Peer::new(&net, "student");
    let mut oracle = Peer::new(&net, "Oracle");
    let (cid, gen) = community_with(&net, &mut student, &mut [&mut oracle]);
    let oracle_id = oracle.acct();
    let planner = Planner::new(vec![R { id: "7:2", text: "HIST 398: exam in 2 days.", stale: false, muted: false }]);
    let srv = planner.serve();
    let tdir = tempfile::tempdir().unwrap();
    let tf = token_file(tdir.path(), TOKEN);

    // the owner switches Oracle off in that room (the signed look)
    student.c.set_oracle_quiet(&cid, &gen, true).unwrap();
    pump(&mut student, &mut oracle);

    let shared = Mutex::new(oracle.c);
    let mut r = Runner::new(Box::new(HttpStudy::new(&srv.url(), &tf)), &gen);
    assert_eq!(r.tick(&mut SharedClient(&shared)), Outcome::Quiet);
    assert!(planner.sent.lock().unwrap().is_empty(), "a reminder nobody saw is not recorded as sent");
    oracle.c = shared.into_inner().unwrap();
    pump(&mut student, &mut oracle);
    assert!(oracle_posts(&student, &gen, &oracle_id).is_empty());
}
