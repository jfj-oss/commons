//! Tests only: a line-JSON control port on 127.0.0.1 for the real window, opened only when `MEGA_GPUI_UI_TEST=<port>` is
//! set (the window's counterpart of `calls/testhook.rs`, which serves the headless engine). One JSON object per line in, one
//! per line out. The window's own clock answers (a quarter of a second), so everything runs on the UI thread, exactly as
//! a click would.
//!   {"cmd":"dump"}                              the ring cards, missed calls, rune bar and the call, as the window holds them
//!   {"cmd":"accept"} {"cmd":"decline"}          the first ring card
//!   {"cmd":"dial","peer","name"}                call a person (what "Call" on a direct conversation does)
//!   {"cmd":"attend","on":false}               behave as a window nobody looks at (so notifications show)
//!   {"cmd":"ring_all"}                          the ring button of the call bar
//!   {"cmd":"join","name"}                       join the voice room with this name
//!   {"cmd":"home"}  {"cmd":"server","name"}  {"cmd":"settings"}   back to the home screen; a community's rooms; settings
//!   {"cmd":"leave"}  {"cmd":"open","name"}      leave the call; open a room (or a direct conversation) by name
//!   {"cmd":"rune","what":"pause|resume|next|prev"}  {"cmd":"along"}   the rune controls of the call bar
//!   {"cmd":"panel","which","name"}               open people|person|manage|invite|join|new|safety|close for a community (by name; person: also "who")
//!   {"cmd":"do","what",...}                     what the panels' buttons do (members.rs `ui_do`): role kick unban transfer leave revoke rename room_* colours wall_* invite verify
//!   {"cmd":"members"}                          what the panel and the communities hold
//!   {"cmd":"click","what"}                     press a panel button: confirm | cancel | create-invite
//!   {"cmd":"core","method","args"}              a call to the core as this account
//!   {"cmd":"paint"}  {"cmd":"paint_do","what",...}   the colours panel: open/close it; hex value | mode card/back | well 0/1 | preset n | tile which n | dim n | oracle quiet/draw | reset | same
//!   {"cmd":"stir"}  {"cmd":"bd"}   a new message's ripple in the open room; the room backdrop's clock, heat and ripples
//!   {"cmd":"draw","on"}  {"cmd":"tool","brush","col"}  {"cmd":"pen","points":[[x,y],...]}  {"cmd":"wall"}   drawing mode, the brush, a stroke in window pixels, what the wall holds
//!   {"cmd":"wall_do","what"}                     lock | clear (the owner's buttons; clear asks twice)
use crate::app::Mega;
use crate::look;
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
                "server" => {
                    let want = s("name");
                    let c = self.snap.as_ref().and_then(|sn| sn.communities.iter().find(|c| c.name == want || want.is_empty()).map(|c| c.id.clone()));
                    match c { Some(cid) => { self.go(crate::app::Level::Server(cid), window, cx); json!({"ok": true}) } None => json!({"error": "no such community"}) }
                }
                "settings" => { self.go(crate::app::Level::Settings, window, cx); json!({"ok": true}) }
                "stir" => { self.stir(2); cx.notify(); json!({"ok": true}) }
                "bd" => json!({"t": self.bd.t, "heat": self.bd.heat, "ripples": self.bd.rips.len(), "live": self.bd.live(), "still": self.still(), "dim": self.gid.as_deref().map(|g| self.bd_dim(g))}),
                "home" => { self.go(crate::app::Level::Home, window, cx); json!({"ok": true}) }
                "attend" => { self.ring.unattended = cmd["on"].as_bool() == Some(false); json!({"ok": true}) }
                "ring_all" => { self.ring_all(window, cx); json!({"ok": true}) }
                "join" => {
                    let want = s("name");
                    let g = self.snap.as_ref().and_then(|sn| sn.communities.iter().find_map(|c| c.channels.iter().find(|ch| ch.name == want).map(|ch| ch.gid.clone())));
                    match g { Some(g) => { self.join_call(g, window, cx); json!({"ok": true}) } None => json!({"error": "no such room"}) }
                }
                "leave" => { self.leave_call(cx); json!({"ok": true}) }
                // the open room's list: scroll it by pixels (positive = towards older), and say how the reading zone drew each visible row
                "scroll" => { let mut dy = cmd["dy"].as_f64().unwrap_or(0.) as f32; if dy > 0. && self.reading.remaining < 1. { dy += self.reading.size.1; } if cmd["end"].as_bool() == Some(true) { self.list.scroll_to_end(); } else { self.list.scroll_by(px(-dy)); } cx.notify(); json!({"ok": true}) }
                "reading" => self.reading_dump(),
                "selection" => self.sel_dump(),
                "clipboard" => json!({"text": cx.read_from_clipboard().and_then(|c| c.text())}),
                "open" => {
                    let want = s("name");
                    let f = self.snap.as_ref().and_then(|sn| sn.direct.iter().find(|d| d.name == want).map(|d| (d.id.clone(), d.room.clone()))
                        .or_else(|| sn.communities.iter().find_map(|c| c.channels.iter().find(|ch| ch.name == want).map(|ch| (c.id.clone(), ch.gid.clone())))));
                    match f { Some((cid, gid)) => { self.open_room(cid, gid, window, cx); json!({"ok": true}) } None => json!({"error": "no such room"}) }
                }
                "rune" => { let what = match s("what").as_str() { "pause" => "pause", "resume" => "resume", "next" => "next", "prev" => "prev", _ => "" }; if what.is_empty() { json!({"error": "bad control"}) } else { self.rune_control(what, window, cx); json!({"ok": true}) } }
                "along" => { self.rune_along(window, cx); json!({"ok": true}) }
                "panel" => {
                    use crate::members::Panel;
                    let want = s("name");
                    let cid = self.snap.as_ref().and_then(|sn| sn.communities.iter().find(|c| c.name == want).map(|c| c.id.clone())).unwrap_or_default();
                    let who = s("who");
                    let acct = self.mem.members.iter().find(|m| m.name == who).map(|m| m.account.clone()).unwrap_or_default();
                    let p = match s("which").as_str() {
                        "people" => Some(Panel::People(cid)), "person" => Some(Panel::Person(cid, acct)), "manage" => Some(Panel::Admin(cid)), "invite" => Some(Panel::Invite(cid)),
                        "join" => Some(Panel::Join), "new" => Some(Panel::NewCommunity), "close" => { self.close_panel(cx); None } _ => None,
                    };
                    if let Some(p) = p { self.open_panel(p, window, cx); }
                    json!({"ok": true})
                }
                "members" => self.members_dump(),
                "nav" => self.nav_dump(),
                "rail" => self.rail_dump(),
                "navplay" => { let now = self.epoch.elapsed().as_secs_f64(); self.nav.frames.clear(); self.nav.last_frame = None; self.nav.play = Some((now + cmd["secs"].as_f64().unwrap_or(3.), cmd["pps"].as_f64().unwrap_or(2400.) as f32, 1.)); cx.notify(); json!({"ok": true}) }
                "railopen" => { if cmd["on"].as_bool() == Some(false) { self.rail.hover = false; self.rail_close_now(); } else { self.rail_open(); } cx.notify(); json!({"ok": true}) }
                "wheel" => { let ev = ScrollWheelEvent { delta: if cmd["lines"].as_f64().is_some() { ScrollDelta::Lines(point(0., -(cmd["lines"].as_f64().unwrap_or(0.) as f32))) } else { ScrollDelta::Pixels(point(px(0.), px(-(cmd["dy"].as_f64().unwrap_or(0.) as f32)))) }, ..Default::default() }; self.nav_wheel(&ev, window, cx); json!({"ok": true}) }
                "navkey" => { let k = s("key"); { let ev = KeyDownEvent { keystroke: Keystroke { modifiers: Default::default(), key: k.clone(), key_char: None }, is_held: false, prefer_character_input: false }; self.nav_key(&ev, window, cx); } json!({"ok": true}) }
                "navclick" => { let i = cmd["i"].as_u64().unwrap_or(0) as usize; self.nav_click(i, window, cx); json!({"ok": true}) }
                "do" => self.ui_do(&cmd, window, cx),
                "paint" => { self.paint_toggle(window, cx); json!({"open": self.paint.open}) }
                "paint_do" => self.paint_do(&cmd, window, cx),
                "draw" => { match cmd["on"].as_bool() { Some(true) => self.wall_on(window, cx), Some(false) => self.wall_off(), None => self.wall_toggle(window, cx) } cx.notify(); json!({"on": self.wall.on, "key": self.wall.key}) }
                "tool" => { if let Some(b) = cmd["brush"].as_u64() { self.wall.brush = b as u8; } if let Some(c) = cmd["col"].as_u64() { self.wall.col = c as u8; } json!({"ok": true}) }
                "pen" => {
                    let pts: Vec<(f32, f32)> = cmd["points"].as_array().map(|l| l.iter().filter_map(|p| Some((p[0].as_f64()? as f32, p[1].as_f64()? as f32))).collect()).unwrap_or_default();
                    for (i, (x, y)) in pts.iter().enumerate() { self.wall_pen(point(px(*x), px(*y)), i == 0, window, cx); }
                    self.wall_pen_up();
                    json!({"ok": true, "pending": self.wall.pend.len()})
                }
                "wall" => self.wall_dump(),
                "wall_do" => { match s("what").as_str() { "lock" => self.wall_lock(window, cx), "clear" => { self.wall_clear(window, cx); self.wall_clear(window, cx); } _ => {} } json!({"ok": true}) }
                "go" => { let want = s("name"); let cid = self.snap.as_ref().and_then(|sn| sn.communities.iter().find(|c| c.name == want).map(|c| c.id.clone())); match cid { Some(c) => { self.go(crate::app::Level::Server(c), window, cx); json!({"ok": true}) } None => json!({"error": "no such community"}) } }
                "join_text" => { let v = s("text"); self.mem.join_in.update(cx, |i, cx| i.set_value(v, window, cx)); self.join_submit(window, cx); json!({"ok": true}) }
                "click" => match s("what").as_str() { "confirm" => { self.run_confirm(window, cx); json!({"ok": true}) } "cancel" => { self.mem.confirm = None; cx.notify(); json!({"ok": true}) } _ => json!({"error": "unknown button"}) },
                "core" => { let core = self.core.clone(); let (m, a) = (s("method"), cmd["args"].clone()); match core.call(&m, a) { Ok(v) => json!({"ok": v}), Err(e) => json!({"error": e}) } }
                other => json!({"error": format!("unknown command {other}")}),
            };
            let _ = tx.send(out);
        }
    }
}

impl Mega {
    /// What the colours panel's controls do, for the test port.
    fn paint_do(&mut self, cmd: &Value, window: &mut Window, cx: &mut Context<Self>) -> Value {
        let s = |k: &str| cmd[k].as_str().unwrap_or("").to_string();
        let n = cmd["n"].as_u64().unwrap_or(0) as usize;
        match s("what").as_str() {
            "hex" => { let v = s("value"); self.paint.hex.update(cx, |h, cx| h.set_value(v, window, cx)); self.paint_hex_changed(window, cx); }
            "mode" => { self.paint.back_mode = s("value") == "back"; self.paint_set_well(self.paint.well, window, cx); }
            "well" => self.paint_set_well(n, window, cx),
            "preset" => self.paint_preset(n, window, cx),
            "tile" => { let which: &'static str = if s("which") == "chat" { "chat" } else { "card" }; self.paint_pattern(which, if cmd["n"].is_null() { None } else { Some(n) }, cx) }
            "dim" => { let v = cmd["value"].as_u64().unwrap_or(100) as u32; self.paint_set_dim(v, cx); }
            "oracle" => self.paint_oracle(s("value") == "draw", window, cx),
            "reset" => self.paint_reset(window, cx),
            "same" => { self.paint_same(window, cx); }
            _ => return json!({"error": "unknown paint action"}),
        }
        cx.notify();
        let (m, h) = self.paint_cur();
        json!({"ok": true, "open": self.paint.open, "cur": [look::hex(m), look::hex(h)], "back": self.paint.back_mode, "well": self.paint.well, "target": self.paint.target.as_ref().map(|t| t.name.clone()), "queue": self.paint_queue_len()})
    }
}
