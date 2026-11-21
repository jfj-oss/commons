//! Tests only: a line-JSON control port on 127.0.0.1 for the real window, opened only when `MEGA_GPUI_UI_TEST=<port>` is
//! set (the window's counterpart of `calls/testhook.rs`, which serves the headless engine). One JSON object per line in, one
//! per line out. The window's own clock answers (a quarter of a second), so everything runs on the UI thread, exactly as
//! a click would.
//!   {"cmd":"dump"}                              the ring cards, missed calls, rune bar and the call, as the window holds them
//!   {"cmd":"accept"} {"cmd":"decline"}          the first ring card
//!   {"cmd":"dial","peer","name"}                call a person (what "Call" on a direct conversation does)
//!   {"cmd":"attend","on":false}               behave as a window nobody looks at (so notifications show)
//!   {"cmd":"room_menu","name"} {"cmd":"room_pick","kind","secs"} {"cmd":"alerts"}   open the notification menu of a room by name (what a right-click does); press a row of it (mute|mentions|none); what the window holds about alerts, the sound and reading size choices
//!   {"cmd":"pref","sound","read_size"}     set the sound (all|mentions|off) and/or the reading size (small|normal|large) for this run, as Settings does
//!   {"cmd":"ring_all"}                          the ring button of the call bar
//!   {"cmd":"join","name"}                       join the voice room with this name
//!   {"cmd":"avatars"} {"cmd":"avatar_open","path"} {"cmd":"avatar_crop","zoom","x","y","circle"} {"cmd":"avatar_use"} {"cmd":"avatar_remove"} {"cmd":"avatar_poll"}   profile pictures: what is held and what the cropper shows; choose a file (what a drop does); crop; send; remove; look now
//!   {"cmd":"files_dump"} files_send{paths} files_clip_image{path} files_paste files_pause/cancel/keep{name} files_open/play/seek/download{name} files_save{folder} files_light{do} files_drag{on} files_resume{name,path} files_drop_parked{id}   files, pictures and sound (files.rs)
//!   {"cmd":"home"}  {"cmd":"server","name"}  {"cmd":"settings"}   back to the home screen; a community's rooms; settings
//!   {"cmd":"leave"}  {"cmd":"open","name"}      leave the call; open a room (or a direct conversation) by name
//!   {"cmd":"rune","what":"pause|resume|next|prev"}  {"cmd":"along"}   the rune controls of the call bar
//!   {"cmd":"panel","which","name"}               open people|person|manage|invite|join|new|safety|close for a community (by name; person: also "who")
//!   {"cmd":"do","what",...}                     what the panels' buttons do (members.rs `ui_do`): role kick unban transfer leave revoke rename room_* colours wall_* invite verify
//!   {"cmd":"members"}                          what the panel and the communities hold
//!   {"cmd":"click","what"}                     press a panel button: confirm | cancel | create-invite
//!   {"cmd":"core","method","args"}              a call to the core as this account
//!   {"cmd":"paint"}  {"cmd":"paint_do","what",...}   the colours panel: open/close it; hex value | mode card/back | well 0/1 | preset n | tile which n | dim n | oracle quiet/draw | reset | same
//!   {"cmd":"saver","mode":"on|off|auto"}  {"cmd":"hidden","on"}   the battery saver for this run (not saved); pretend the window is hidden
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

macro_rules! return_err { ($tx:expr, $m:expr) => {{ let _ = $tx.send(json!({"error": $m})); continue; }}; }

impl Mega {
    pub fn uitest_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(q) = self.uitest.clone() else { return };
        let jobs: Vec<Job> = q.0.lock().unwrap().drain(..).collect();
        for (cmd, tx) in jobs {
            let s = |k: &str| cmd[k].as_str().unwrap_or("").to_string();
            let out = match cmd["cmd"].as_str().unwrap_or("") {
                "dump" => json!({"ring": self.ring_dump(), "rune": self.rune_dump(), "call": {"phase": self.call.snap.phase, "gid": self.call.snap.gid, "peers": self.call.snap.peers.iter().map(|p| json!({"name": p.name, "account": p.account, "me": p.me})).collect::<Vec<_>>()},
                                "level": format!("{:?}", self.level), "gid": self.gid, "active": window.is_window_active()}),
                "callsnap" => { let mut v = self.callsnap(); v["windows"] = json!(cx.windows().len()); let vp = window.viewport_size(); v["viewport"] = json!([f32::from(vp.width), f32::from(vp.height)]); v["bounds"] = json!({"x": f32::from(window.bounds().origin.x), "y": f32::from(window.bounds().origin.y), "w": f32::from(window.bounds().size.width), "h": f32::from(window.bounds().size.height)}); v }
                "call_settings" => { if self.call.settings_open != (cmd["on"].as_bool() != Some(false)) { self.open_settings(window, cx); } json!({"ok": true}) }
                "accept" => match self.ring.book.cards.first().map(|c| c.ring.clone()) { Some(r) => { self.ring_accept(&r, window, cx); json!({"ok": true}) } None => json!({"error": "no card"}) },
                "decline" => match self.ring.book.cards.first().map(|c| c.ring.clone()) { Some(r) => { self.ring_decline(&r, cx); json!({"ok": true}) } None => json!({"error": "no card"}) },
                "ob" => {
                    // the first screens: {"cmd":"ob","what":"dump|mode|set|go|copy_key|over"...}
                    use crate::onboard::Boot;
                    match s("what").as_str() {
                        "dump" => self.ob_dump(cx),
                        "mode" => { let m = match s("mode").as_str() { "pw" => Boot::Invite, "new" => Boot::Create, "pair" => Boot::Pair, _ => Boot::Recover }; self.ob_show(m, window, cx); json!({"ok": true}) }
                        "set" => {
                            let v = s("value");
                            let e = match s("field").as_str() { "ipw" => &self.ob.ipw, "link" => &self.ob.link, "name" => &self.ob.name, "pp" => &self.ob.pp, "pp2" => &self.ob.pp2, "relay" => &self.ob.relay, "secret" => &self.ob.secret, "rk" => &self.ob.rk, _ => return_err!(tx, "no such field") }.clone();
                            e.update(cx, |i, cx| i.set_value(v, window, cx));
                            if s("field") == "link" { self.ob_read_link(window, cx); }
                            if s("field") == "pp" { self.ob.hint = crate::onboard::pp_strength(&s("value")); }
                            json!({"ok": true})
                        }
                        "go" => { self.ob_go(window, cx); json!({"ok": true}) }
                        "over" => { self.ob_start_over(window, cx); json!({"ok": true}) }
                        "relay_open" => { self.ob.relay_open = true; cx.notify(); json!({"ok": true}) }
                        _ => json!({"error": "unknown ob action"}),
                    }
                }
                "dev" => {
                    // devices, key, log, relay: {"cmd":"dev","what":"dump|open|code|approve|revoke|history|key|key_saved|relay|refuse"...}
                    use crate::members::Panel;
                    match s("what").as_str() {
                        "dump" => self.dev_dump(cx),
                        "open" => { let p = match s("which").as_str() { "key" => Panel::Key, "log" => Panel::Activity, _ => Panel::Devices }; self.dev_open(p, window, cx); json!({"ok": true}) }
                        "close" => { self.close_panel(cx); json!({"ok": true}) }
                        "code" => { let v = s("value"); self.dev.code.update(cx, |i, cx| i.set_value(v, window, cx)); self.dev_preview(window, cx); json!({"ok": true}) }
                        "history_on" => { self.dev.send_history = cmd["on"].as_bool() != Some(false); json!({"ok": true}) }
                        "approve" => { self.dev_approve(window, cx); json!({"ok": true}) }
                        "revoke" => { self.dev_revoke(s("device"), window, cx); json!({"ok": true}) }
                        "history" => { let m = match s("method").as_str() { "send" => "history_send", "decline" => "history_decline", _ => "history_dismiss" }; self.dev_history(m, s("device"), window, cx); json!({"ok": true}) }
                        "key_pw" => { let v = s("value"); self.dev.key_pw.update(cx, |i, cx| i.set_value(v, window, cx)); self.dev_key_show(window, cx); json!({"ok": true}) }
                        "key_saved" => { self.dev_key_saved(window, cx); json!({"ok": true}) }
                        "relay_set" => { let (r, k) = (s("relay"), s("secret")); self.dev.relay.update(cx, |i, cx| i.set_value(r, window, cx)); self.dev.relay_secret.update(cx, |i, cx| i.set_value(k, window, cx)); self.dev_relay_save(window, cx); json!({"ok": true}) }
                        "refuse" => { self.direct_refuse(s("account"), s("name"), cmd["on"].as_bool() != Some(false), window, cx); json!({"ok": true}) }
                        _ => json!({"error": "unknown dev action"}),
                    }
                }
                "dial" => { self.dial(s("peer"), s("name"), window, cx); json!({"ok": true}) }
                "server" => {
                    let want = s("name");
                    let c = self.snap.as_ref().and_then(|sn| sn.communities.iter().find(|c| c.name == want || want.is_empty()).map(|c| c.id.clone()));
                    match c { Some(cid) => { self.go(crate::app::Level::Server(cid), window, cx); json!({"ok": true}) } None => json!({"error": "no such community"}) }
                }
                "settings" => { self.go(crate::app::Level::Settings, window, cx); json!({"ok": true}) }
                "saver" => { self.prefs.saver = match s("mode").as_str() { "on" => crate::prefs::Saver::On, "off" => crate::prefs::Saver::Off, _ => crate::prefs::Saver::Auto }; cx.notify(); json!({"ok": true, "still": self.still()}) }
                "hidden" => { self.fake_hidden = cmd["on"].as_bool() == Some(true); json!({"ok": true}) }
                "stir" => { self.stir(2); cx.notify(); json!({"ok": true}) }
                "bd" => json!({"t": self.bd.t, "heat": self.bd.heat, "ripples": self.bd.rips.len(), "busy": self.bd.busy(), "next_rip": self.bd.next_rip, "visible": window.is_visible(), "typing": self.win.typing, "still": self.still(), "dim": self.gid.as_deref().map(|g| self.bd_dim(g))}),
                "avatars" => self.avatar_dump(),
                "avatar_open" => { self.avatar_open(std::path::PathBuf::from(s("path")), window, cx); json!({"ok": true}) }
                "avatar_crop" => {
                    let (z, x, y) = (cmd["zoom"].as_f64(), cmd["x"].as_f64(), cmd["y"].as_f64());
                    let circle = cmd["circle"].as_bool();
                    self.avatar_crop_set(|c| { if let Some(z) = z { c.zoom = z as f32; } if let Some(x) = x { c.x = x as f32; } if let Some(y) = y { c.y = y as f32; } if let Some(b) = circle { c.circle = b; } }, cx);
                    self.avatar_dump()
                }
                "avatar_use" => { self.avatar_use(window, cx); json!({"ok": true}) }
                "avatar_remove" => { self.avatar_remove(window, cx); json!({"ok": true}) }
                "avatar_poll" => { self.avatar_poll(); json!({"ok": true}) }
                // the cards of the current screen as drawn: title, unread, mentions (`@N`)
                "cards" => {
                    let items = match (&self.level, self.snap.as_ref()) {
                        (crate::app::Level::Home, _) => self.community_items_pub(),
                        (crate::app::Level::Server(cid), Some(sn)) => sn.communities.iter().find(|c| &c.id == cid).map(|c| self.room_items(c)).unwrap_or_default(),
                        _ => vec![],
                    };
                    json!({"cards": items.iter().map(|i| json!({"title": i.title, "num": i.num, "at": i.at, "voice": i.voice})).collect::<Vec<_>>(), "level": format!("{:?}", self.level)})
                }
                // chat density: set the stop of the open room (and "all" for every room) and say what it gives
                "density" => {
                    if let Some(n) = cmd["level"].as_f64() { if let Some(g) = self.gid.clone() { if let Some(a) = cmd["all"].as_bool() { let shown = self.density_now(); self.prefs.set_dens_all(&g, a, shown); } self.set_density(crate::density::clamp(n as f32, self.prefs.chat_font == crate::prefs::ChatFont::Raster), cx); } }
                    if let Some(f) = cmd["font"].as_str() { self.prefs.chat_font = if f == "raster" { crate::prefs::ChatFont::Raster } else { crate::prefs::ChatFont::Bellefair }; self.list.remeasure(); self.reading.full.clear(); }
                    if let Some(m) = cmd["menu"].as_bool() { if m != self.dens.open { self.dens_toggle(window, cx); } }
                    json!({"level": self.density_now(), "all": self.prefs.dens_all, "params": self.dens_params().map(|d| json!({"base": d.base, "tier": d.tier, "gap": d.gap, "pad": d.pad})), "menu": self.dens.open, "hits": self.dens.hits.dump()})
                }
                "gk" => {
                    match cmd["what"].as_str().unwrap_or("") {
                        "open" => self.gk_open(window, cx),
                        "close" => self.gk_close(cx),
                        "add" => { let n = s("name"); self.gk.d.add_program(&n); cx.notify(); }
                        _ => {}
                    }
                    json!({"open": self.gk.open, "rows": self.gk.d.rows.iter().map(|r| json!({"process": r.process, "key": r.key, "mode": r.mode})).collect::<Vec<_>>(), "any": {"key": self.gk.d.any.key, "mode": self.gk.d.any.mode}, "capture": self.gk.capture, "error": self.gk.error, "programs": self.gk.programs, "portal": self.gk.d.portal, "note": self.gk.d.note, "hits": self.gk.hits.dump()})
                }
                "tour" => {
                    match cmd["what"].as_str().unwrap_or("") {
                        "start" => self.tour_start(window, cx),
                        "end" => self.tour_end(true, cx),
                        "reset" => { self.prefs.tour_seen = false; self.prefs.save(); }
                        _ => {}
                    }
                    self.tour_dump()
                }
                "screen" => json!({"screen": format!("{:?}", self.screen), "error": self.error.as_ref().map(|e| e.to_string()), "dir": self.core.dir.display().to_string(), "level": format!("{:?}", self.level)}),
                "bloom" => json!({"bloom": [self.nav.bloom.0, self.nav.bloom.1, self.nav.bloom.2], "cur": self.nav.scroll.cur}),
                "home" => { self.go(crate::app::Level::Home, window, cx); json!({"ok": true}) }
                "attend" => { self.ring.unattended = cmd["on"].as_bool() == Some(false); json!({"ok": true}) }
                "room_menu" => {
                    let want = s("name");
                    let g = self.snap.as_ref().and_then(|sn| sn.communities.iter().chain(sn.direct.iter()).find_map(|c| c.channels.iter().find(|ch| ch.name == want || c.name == want && c.direct).map(|ch| (ch.gid.clone(), ch.name.clone()))));
                    match g { Some((g, n)) => { self.room_menu_open(&g, &n, point(px(300.), px(200.)), window, cx); json!({"ok": true, "gid": g}) } None => json!({"error": "no such room"}) }
                }
                "room_pick" => { self.room_menu_pick(&s("kind"), cmd["secs"].as_u64().unwrap_or(0), window, cx); json!({"ok": true}) }
                "alerts" => self.alerts_dump(),
                "pref" => {
                    use crate::prefs::{ReadSize, Sound};
                    match cmd["sound"].as_str() { Some("all") => self.prefs.msg_sound = Sound::All, Some("mentions") => self.prefs.msg_sound = Sound::Mentions, Some("off") => self.prefs.msg_sound = Sound::Off, _ => {} }
                    match cmd["read_size"].as_str() { Some("small") => self.prefs.read_size = ReadSize::Small, Some("normal") => self.prefs.read_size = ReadSize::Normal, Some("large") => self.prefs.read_size = ReadSize::Large, _ => {} }
                    self.list.remeasure(); self.reading.full.clear(); cx.notify();
                    self.alerts_dump()
                }
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
                // message actions without a pointer: msg (what the composer holds and where the tools are) and msg_do {what: reply|edit|delete|react|goto|older|cancel, id, emoji}
                "msg" => self.msg_dump(),
                "search" => self.search_dump(),
                "emoji" => self.emoji_dump(),
                "emoji_do" => { match cmd["what"].as_str().unwrap_or("") { "open" => { self.emoji_open(crate::emoji::Target::Composer, point(px(0.), px(0.)), window, cx); } "close" => self.emoji_close(window, cx), "query" => { let q = s("q"); self.emoji.input.update(cx, |i, cx| i.set_value(q, window, cx)); self.emoji_rebuild(cx); } _ => {} } json!({"ok": true}) }
                "search_do" => { match cmd["what"].as_str().unwrap_or("") { "open" => self.search_open(window, cx), "close" => self.search_close(window, cx), _ => {} } json!({"ok": true}) }
                "msg_do" => {
                    let id = s("mid");
                    match cmd["what"].as_str().unwrap_or("") {
                        "reply" => self.msg_reply(&id, window, cx), "edit" => self.msg_edit(&id, window, cx), "delete" => self.msg_delete(&id, window, cx),
                        "react" => self.msg_react(&id, &s("emoji"), window, cx), "goto" => self.msg_goto(&id, window, cx), "older" => self.msg_older(window, cx), "cancel" => self.msg_cancel(window, cx),
                        o => { let o = o.to_string(); let _ = tx.send(json!({"error": format!("unknown {o}")})); continue; }
                    }
                    json!({"ok": true})
                }
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
                "paint_for" => { self.paint_open_for(&s("cid"), &s("room"), window, cx); self.paint_state() }
                "paint_state" => self.paint_state(),
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
                other if other.starts_with("files_") => self.files_cmd(&cmd, window, cx),
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
        let _ = (m, h);
        self.paint_state()
    }
    /// The panel as it is: the colours, the warning, the square and hue strip in window pixels (for real pointer events).
    fn paint_state(&mut self) -> Value {
        let (m, h) = self.paint_cur();
        let r = |b: Bounds<Pixels>| json!([f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)]);
        json!({"ok": true, "open": self.paint.open, "external": self.paint.external, "cur": [look::hex(m), look::hex(h)], "warn": look::contrast(m, h) < 1.6, "hsv": self.paint.hsv, "back": self.paint.back_mode, "well": self.paint.well,
            "target": self.paint.target.as_ref().map(|t| t.name.clone()), "queue": self.paint_queue_len(), "sv": r(self.paint.sv.get()), "hue": r(self.paint.hue.get())})
    }
}
