//! Tests only: a line-JSON control port on 127.0.0.1 for the real window, opened only when `MEGA_GPUI_UI_TEST=<port>` is
//! set (the window's counterpart of `calls/testhook.rs`, which serves the headless engine). One JSON object per line in, one
//! per line out. The window's own clock answers (a quarter of a second), so everything runs on the UI thread, exactly as
//! a click would.
//!   {"cmd":"dump"}                              the ring cards, missed calls, rune bar and the call, as the window holds them
//!   {"cmd":"accept"} {"cmd":"decline"}          the first ring card
//!   {"cmd":"dial","peer","name"}                call a person (what "Call" on a direct conversation does)
//!   {"cmd":"ring_all"}                          the ring button of the call bar
//!   {"cmd":"join","name"}                       join the voice room with this name
//!   {"cmd":"leave"}  {"cmd":"open","name"}      leave the call; open a room (or a direct conversation) by name
//!   {"cmd":"rune","what":"pause|resume|next|prev"}  {"cmd":"along"}   the rune controls of the call bar
//!   {"cmd":"core","method","args"}              a call to the core as this account
use crate::app::Mega;
use gpui_kit::*;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub type Job = (Value, Sender<Value>);
#[derive(Default)]
pub struct Queue(Mutex<VecDeque<Job>>);

pub fn from_env() -> Option<Arc<Queue>> {
    let port: u16 = std::env::var("MEGA_GPUI_UI_TEST").ok()?.parse().ok()?;
    let q = Arc::new(Queue::default());
    let q2 = q.clone();
    let listener = std::net::TcpListener::bind(("127.0.0.1", port)).map_err(|e| eprintln!("ui test port: {e}")).ok()?;
    std::thread::Builder::new().name("mega-uitest".into()).spawn(move || {
        for sock in listener.incoming().flatten() {
            let q = q2.clone();
            std::thread::spawn(move || {
                let mut w = match sock.try_clone() { Ok(w) => w, Err(_) => return };
                for line in BufReader::new(sock).lines().map_while(Result::ok) {
                    let cmd: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                    let id = cmd["id"].clone();
                    let (tx, rx) = channel();
                    q.0.lock().unwrap().push_back((cmd, tx));
                    let mut out = rx.recv_timeout(Duration::from_secs(20)).unwrap_or_else(|_| json!({"error": "the window did not answer"}));
                    out["id"] = id;
                    if writeln!(w, "{out}").is_err() { return; }
                }
            });
        }
    }).ok()?;
    Some(q)
}

impl Mega {
    pub fn uitest_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(q) = self.uitest.clone() else { return };
        let jobs: Vec<Job> = q.0.lock().unwrap().drain(..).collect();
        for (cmd, tx) in jobs {
            let s = |k: &str| cmd[k].as_str().unwrap_or("").to_string();
            let out = match cmd["cmd"].as_str().unwrap_or("") {
                "dump" => json!({"ring": self.ring_dump(), "rune": self.rune_dump(), "call": {"phase": self.call.snap.phase, "gid": self.call.snap.gid, "peers": self.call.snap.peers.iter().map(|p| json!({"name": p.name, "account": p.account, "me": p.me})).collect::<Vec<_>>()},
                                "level": format!("{:?}", self.level), "gid": self.gid, "active": window.is_window_active()}),
                "accept" => match self.ring.book.cards.first().map(|c| c.ring.clone()) { Some(r) => { self.ring_accept(&r, window, cx); json!({"ok": true}) } None => json!({"error": "no card"}) },
                "decline" => match self.ring.book.cards.first().map(|c| c.ring.clone()) { Some(r) => { self.ring_decline(&r, cx); json!({"ok": true}) } None => json!({"error": "no card"}) },
                "dial" => { self.dial(s("peer"), s("name"), window, cx); json!({"ok": true}) }
                "ring_all" => { self.ring_all(window, cx); json!({"ok": true}) }
                "join" => {
                    let want = s("name");
                    let g = self.snap.as_ref().and_then(|sn| sn.communities.iter().find_map(|c| c.channels.iter().find(|ch| ch.name == want).map(|ch| ch.gid.clone())));
                    match g { Some(g) => { self.join_call(g, window, cx); json!({"ok": true}) } None => json!({"error": "no such room"}) }
                }
                "leave" => { self.leave_call(cx); json!({"ok": true}) }
                "open" => {
                    let want = s("name");
                    let f = self.snap.as_ref().and_then(|sn| sn.direct.iter().find(|d| d.name == want).map(|d| (d.id.clone(), d.room.clone()))
                        .or_else(|| sn.communities.iter().find_map(|c| c.channels.iter().find(|ch| ch.name == want).map(|ch| (c.id.clone(), ch.gid.clone())))));
                    match f { Some((cid, gid)) => { self.open_room(cid, gid, window, cx); json!({"ok": true}) } None => json!({"error": "no such room"}) }
                }
                "rune" => { let what = match s("what").as_str() { "pause" => "pause", "resume" => "resume", "next" => "next", "prev" => "prev", _ => "" }; if what.is_empty() { json!({"error": "bad control"}) } else { self.rune_control(what, window, cx); json!({"ok": true}) } }
                "along" => { self.rune_along(window, cx); json!({"ok": true}) }
                "core" => { let core = self.core.clone(); let (m, a) = (s("method"), cmd["args"].clone()); match core.call(&m, a) { Ok(v) => json!({"ok": v}), Err(e) => json!({"error": e}) } }
                other => json!({"error": format!("unknown command {other}")}),
            };
            let _ = tx.send(out);
        }
    }
}
