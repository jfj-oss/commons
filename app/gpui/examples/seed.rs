//! Throwaway people and rooms on a LOCAL relay, for trying and measuring mega-gpui without touching a real account.
//!   seed setup <relay-url> <dir> [long-room-messages]   two accounts (<dir>/alice, <dir>/bob), two communities, a long room
//!   seed chat  <relay-url> <dir> <room-name> <count> <interval-ms>   bob writes into a room while a window on alice is open
//! The passphrase of both throwaway accounts is PASS below. Never point this at a relay people use.
use commons_core::api::App;
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::{Duration, Instant}};

pub const PASS: &str = "throwaway-pass-1";

fn call(app: &Arc<App>, m: &str, a: Value) -> Value { app.call(m, a).unwrap_or_else(|e| panic!("{m}: {e}")) }
fn wait_for(what: &str, secs: u64, mut ok: impl FnMut() -> bool) {
    let t = Instant::now();
    while !ok() { if t.elapsed() > Duration::from_secs(secs) { panic!("timed out waiting for {what}"); } std::thread::sleep(Duration::from_millis(100)); }
}
fn open(relay: &str, dir: PathBuf, name: &str) -> Arc<App> {
    assert!(relay.starts_with("http://127.0.0.1:") || relay.starts_with("http://localhost:"), "seed only talks to a relay on this computer");
    let app = App::new(dir.clone(), relay);
    app.start_background();
    if call(&app, "state", json!({}))["exists"].as_bool() == Some(true) { call(&app, "unlock", json!({"passphrase": PASS})); }
    else { call(&app, "create_account", json!({"relay": relay, "passphrase": PASS, "name": name, "invite": ""})); let _ = app.call("recovery_saved", json!({"unsaved": false})); }
    app
}
fn room(app: &Arc<App>, name: &str) -> Option<String> {
    let s = call(app, "snapshot", json!({}));
    s["communities"].as_array()?.iter().flat_map(|c| c["channels"].as_array().cloned().unwrap_or_default()).find(|ch| ch["name"] == name && ch["joined"] == true).and_then(|ch| ch["gid"].as_str().map(String::from))
}
fn drained(app: &Arc<App>) -> bool { call(app, "snapshot", json!({}))["outbox"].as_u64() == Some(0) }

const LINES: [&str; 8] = [
    "did anyone water the ferns?", "I did, on Tuesday. They looked thirsty.", "ok so the plan is: bread, then the lake, then we see",
    "this one is a longer message so that the list has rows of different heights, which is what a real room looks like after a week of people talking about nothing in particular and everything at once",
    "ha", "two lines\nin one message", "日本語のメッセージ、絵文字も 🌿🥖", "see you at 7",
];

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(|s| s.as_str()) {
        Some("setup") => {
            let (relay, dir) = (&a[2], PathBuf::from(&a[3]));
            let long: usize = a.get(4).and_then(|x| x.parse().ok()).unwrap_or(5000);
            let alice = open(relay, dir.join("alice"), "Alice");
            let bob = open(relay, dir.join("bob"), "Bob");
            let hall = call(&alice, "create_community", json!({"name": "Bench Hall"}))["id"].as_str().unwrap().to_string();
            for r in ["long", "kitchen"] { call(&alice, "create_channel", json!({"cid": hall, "name": r})); }
            let _ = alice.call("create_channel", json!({"cid": hall, "name": "porch", "kind": "voice"}));
            let garden = call(&alice, "create_community", json!({"name": "Garden"}))["id"].as_str().unwrap().to_string();
            call(&alice, "create_channel", json!({"cid": garden, "name": "seeds"}));
            for cid in [&hall, &garden] {
                let link = call(&alice, "create_invite", json!({"cid": cid, "uses": 3}))["link"].as_str().unwrap().to_string();
                call(&bob, "join", json!({"link": link}));
            }
            wait_for("bob to be let into the rooms", 60, || room(&bob, "long").is_some() && room(&bob, "kitchen").is_some() && room(&bob, "seeds").is_some());
            let (ga, gb) = (room(&alice, "long").unwrap(), room(&bob, "long").unwrap());
            let t = Instant::now();
            for i in 0..long {
                let (app, gid) = if i % 3 == 2 { (&bob, &gb) } else { (&alice, &ga) };
                call(app, "send_text", json!({"gid": gid, "text": format!("{} ({i})", LINES[i % LINES.len()])}));
                if i % 200 == 199 { wait_for("the outbox to drain", 120, || drained(&alice) && drained(&bob)); eprintln!("  {} / {long}", i + 1); }
            }
            let (ka, kb) = (room(&alice, "kitchen").unwrap(), room(&bob, "kitchen").unwrap());
            for i in 0..40 { let (app, gid) = if i % 2 == 0 { (&bob, &kb) } else { (&alice, &ka) }; call(app, "send_text", json!({"gid": gid, "text": LINES[i % LINES.len()]})); }
            wait_for("everything to be sent", 300, || drained(&alice) && drained(&bob));
            // each side has what the other wrote (bob wrote a third of the long room)
            wait_for("alice to have the long room", 300, || call(&alice, "messages", json!({"gid": ga, "limit": 5000}))["messages"].as_array().map(|l| l.len()).unwrap_or(0) >= long.min(5000));
            wait_for("bob to have the long room", 300, || call(&bob, "messages", json!({"gid": gb, "limit": 5000}))["messages"].as_array().map(|l| l.len()).unwrap_or(0) >= long.min(5000));
            // a one-to-one conversation, if this relay and both apps do them
            let me = call(&alice, "snapshot", json!({}))["me"]["account"].as_str().unwrap_or("").to_string();
            if let Err(e) = bob.call("direct_call", json!({"peer": me})) { eprintln!("no direct conversation: {e}"); }
            std::thread::sleep(Duration::from_secs(4));
            eprintln!("seeded {long} messages in {:.1}s", t.elapsed().as_secs_f32());
            for app in [&alice, &bob] { call(app, "lock", json!({})); app.shutdown(); }
        }
        Some("chat") => {
            let (relay, dir, name) = (&a[2], PathBuf::from(&a[3]), &a[4]);
            let (n, ms): (usize, u64) = (a[5].parse().unwrap(), a[6].parse().unwrap());
            let bob = open(relay, dir.join("bob"), "Bob");
            wait_for("the room", 30, || room(&bob, name).is_some());
            let gid = room(&bob, name).unwrap();
            for i in 0..n {
                let _ = bob.call("typing", json!({"gid": gid}));
                std::thread::sleep(Duration::from_millis(ms));
                call(&bob, "send_text", json!({"gid": gid, "text": format!("live {i}: {}", LINES[i % LINES.len()])}));
            }
            wait_for("everything to be sent", 60, || drained(&bob));
            std::thread::sleep(Duration::from_millis(1500));
            call(&bob, "lock", json!({})); bob.shutdown();
        }
        _ => { eprintln!("usage: seed setup <relay> <dir> [n] | seed chat <relay> <dir> <room> <count> <interval-ms>"); std::process::exit(2); }
    }
}
