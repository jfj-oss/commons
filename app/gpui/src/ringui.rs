//! Rings and direct calls in the window (CALLS-PLAN.md step 7): the incoming ring cards, the missed-call notes, calling a
//! person, ringing the room, and the one-thing rule. The rules are in `calls/ring.rs`; the wire is the core's (`ring_poll`,
//! `ring_send`, `ring_end`, `direct_call`), the same as the web's, so a ring from Chromium or iOS shows here and the other
//! way round. Joining is the ordinary encrypted join: the engine ends a call we are in before it joins another.
use crate::app::{Mega, Screen};
use crate::calls::engine::Command;
use crate::calls::ring::{self, Book, Chime, Dial, Ev, Out, Room};
use crate::look;
use crate::notifier::Inbound;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{json, Value};

pub fn unix() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.) }

#[derive(Default)]
pub struct RingUi {
    pub book: Book,
    busy: bool,
    last_poll: f64,
    /// Short notices (what the web shows as a toast): text and when it goes.
    pub notes: Vec<(String, f64)>,
    /// A direct call we asked the core for, waiting for its room to appear in the snapshot: (room, the person's name, give up at).
    pub joining: Vec<(String, String, f64)>,
    chime: Option<Chime>,
    chime_sound: Option<bool>,
    /// Tests only: behave as a window nobody is looking at.
    pub unattended: bool,
    /// Counts for checks.
    pub stats: Stats,
}
#[derive(Default, Clone, Copy, Debug)]
pub struct Stats { pub shown: u32, pub tones: u32, pub polls: u32 }

impl Mega {
    pub fn say(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.ring.notes.push((text.into(), unix() + 6.));
        if self.ring.notes.len() > 3 { self.ring.notes.remove(0); }
        cx.notify();
    }

    /// Is this window the one being looked at? (A test can say it is not: `{"cmd":"attend","on":false}` on the test port.)
    pub fn attended(&self, window: &Window) -> bool { window.is_window_active() && !self.ring.unattended }

    /// The room of the call we are in, whatever its phase.
    fn my_call_gid(&self) -> Option<String> { if self.call.in_call() { self.call.snap.gid.clone() } else { None } }

    fn room_for(&self, gid: &str) -> Option<Room> {
        self.channel(gid).map(|(c, ch)| Room { cid: c.id.clone(), one_to_one: c.direct, room: if c.direct { c.name.clone() } else { ch.name.clone() }, comm: c.name.clone() })
    }

    /// The half-second clock of everything that rings: what notifications ask of the window, the ring poll (once a second),
    /// expiry, and the pending direct calls.
    pub fn ring_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen != Screen::Main { return; }
        let now = unix();
        for x in self.core.inbox.take() {
            match x {
                Inbound::Open(gid) => {
                    window.activate_window();
                    if let Some(g) = gid { if let Some(cid) = self.channel(&g).map(|(c, _)| c.id.clone()) { self.open_room(cid, g, window, cx); } }
                }
                Inbound::Replied { ok, .. } => { if !ok { self.say("The quick reply could not be sent.", cx); } else { self.tick(window, cx); } }
            }
        }
        self.ring.notes.retain(|(_, until)| *until > now);
        self.dials_ready(now, window, cx);
        // expiry and the rules that tie rings to the call we are in
        let my = self.my_call_gid();
        let others = self.call.others();
        let before = self.ring.book.missed.len();
        let snap = self.snap.clone();
        let exists = |g: &str| snap.as_ref().is_some_and(|s| s.communities.iter().chain(s.direct.iter()).any(|c| c.channels.iter().any(|ch| ch.gid == g)));
        let swept = self.ring.book.sweep(now, my.as_deref(), &exists, &|a| others.contains(a));
        for o in &swept.end_outs { self.rpc_end(o, window, cx); }
        for d in swept.no_answer {
            self.say(format!("{} did not answer. If their mega is older or closed it cannot ring.", d.name), cx);
            self.leave_call(cx);
        }
        if self.ring.book.missed.len() > before { self.notify_missed(window); }
        if swept.changed { cx.notify(); }
        // the sound, if due (never in a test: see `Chime::choose`)
        if self.ring.book.tone_due(now) {
            let on = self.prefs.sound;
            if self.ring.chime_sound != Some(on) { self.ring.chime = Some(Chime::choose(on, &self.core.dir)); self.ring.chime_sound = Some(on); }
            if let Some(c) = &self.ring.chime { c.play(&self.ring.book.cards.first().map(|c| c.name.clone()).unwrap_or_default()); }
            self.ring.stats.tones += 1;
        }
        if now - self.ring.last_poll >= 1.0 && !self.ring.busy {
            self.ring.busy = true;
            self.ring.last_poll = now;
            self.ring.stats.polls += 1;
            let since = self.ring.book.since;
            self.rpc("ring_poll", json!({"since": since}), window, cx, |this, r, window, cx| {
                this.ring.busy = false;
                let Ok(r) = r else { return };
                let evs: Vec<Ev> = r["events"].as_array().map(|l| l.iter().filter_map(Ev::parse).collect()).unwrap_or_default();
                for ev in evs { this.ring_event(ev, window, cx); }
            });
        }
    }

    fn ring_event(&mut self, ev: Ev, window: &mut Window, cx: &mut Context<Self>) {
        let now = unix();
        let room = self.room_for(&ev.gid);
        let my = self.my_call_gid();
        let before = self.ring.book.missed.len();
        let made = self.ring.book.take(&ev, room.clone(), my.as_deref(), now);
        if self.ring.book.missed.len() > before { self.notify_missed(window); }
        if !made { cx.notify(); return; }
        self.ring.stats.shown += 1;
        let (ring_id, cid, acct) = (ev.ring.clone(), room.map(|r| r.cid).unwrap_or_default(), ev.account.clone());
        // who it is: the name the room knows them by
        self.rpc("members", json!({"cid": cid}), window, cx, move |this, r, window, cx| {
            let name = r.ok().and_then(|v| v.as_array().and_then(|l| l.iter().find(|m| m["account"].as_str() == Some(acct.as_str())).and_then(|m| m["name"].as_str().map(String::from))));
            if let Some(n) = name { this.ring.book.set_name(&ring_id, &n); }
            if let Some(c) = this.ring.book.cards.iter().find(|c| c.ring == ring_id).cloned() { this.notify_ring(&c, window); }
            cx.notify();
        });
        cx.notify();
    }

    /// A system notification for a ring while the window is not the one being looked at, unless notifications are off.
    fn notify_ring(&mut self, c: &ring::Card, window: &Window) {
        if self.attended(window) || !self.notifications_on() { return; }
        let Some(n) = self.core.notifier.clone() else { return };
        let title = if c.direct { format!("{} is calling you", c.name) } else { format!("{} is calling in {}", c.name, c.room) };
        n.show_plain(&format!("ring:{}", c.ring), &title, &format!("{} · {}", c.room, c.comm), Some(&c.gid));
    }
    fn notify_missed(&mut self, window: &Window) {
        if self.attended(window) || !self.notifications_on() { return; }
        let (Some(n), Some(m)) = (self.core.notifier.clone(), self.ring.book.missed.last().cloned()) else { return };
        n.show_plain(&format!("missed:{}", m.at as u64), "Missed call", &m.line(), None);
    }
    fn notifications_on(&self) -> bool { self.core.app.notify_hub().settings().mode != commons_core::notify::Mode::Off }

    // ---- answering ----
    /// Join from a card. The engine ends the call we are in first (one thing at a time).
    pub fn ring_accept(&mut self, ring: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.ring.book.accept(ring) else { return };
        self.ring.book.answered(&c.gid);
        if let Some(n) = self.core.notifier.clone() { n.close_all(); }
        self.join_named(c.gid, if c.one_to_one { c.name } else { c.room }, window, cx);
    }
    pub fn ring_decline(&mut self, ring: &str, cx: &mut Context<Self>) {
        self.ring.book.decline(ring);
        cx.notify();
    }
    pub fn join_named(&mut self, gid: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let e = self.engine(window, cx);
        e.send(Command::Join { gid, name });
        self.call.snap = e.snapshot();
        cx.notify();
    }

    // ---- ringing ----
    fn rpc_end(&mut self, o: &Out, window: &mut Window, cx: &mut Context<Self>) {
        self.rpc("ring_end", json!({"gid": o.gid, "id": o.id, "to": o.to}), window, cx, |_, _, _, _| {});
    }
    /// The ring button of the call bar: ring everyone in the community who is not in the call, or stop ringing.
    pub fn ring_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(o) = self.ring.book.outs.remove("*") { self.rpc_end(&o, window, cx); cx.notify(); return; }
        let Some(gid) = self.ring_all_room() else { return };
        self.rpc("ring_send", json!({"gid": gid, "to": ""}), window, cx, move |this, r, _, cx| {
            match r {
                Ok(v) => {
                    let id = v["id"].as_str().unwrap_or("").to_string();
                    this.ring.book.outs.insert("*".into(), Out { gid, id, to: String::new(), until: unix() + ring::TTL });
                    this.say("Ringing everyone who is not in the call. People with mega closed are not reached.", cx);
                }
                Err(e) => this.say(e, cx),
            }
            cx.notify();
        });
    }
    /// The voice room of our connected call, if it is one that has others to ring (not a call between two).
    fn ring_all_room(&self) -> Option<String> {
        if !self.call.connected() { return None; }
        let gid = self.call.snap.gid.clone()?;
        self.channel(&gid).filter(|(c, ch)| !c.direct && ch.kind == "voice").map(|_| gid)
    }

    // ---- calling a person ----
    /// "Call" on a person: ask the core for the hidden two-person room (it rings them, or invites them first), then join it.
    pub fn dial(&mut self, peer: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.call.in_call() { self.say("Leave your current call first.", cx); return; }
        if !self.ring.joining.is_empty() { return; }
        self.rpc("direct_call", json!({"peer": peer}), window, cx, move |this, r, _, cx| {
            match r {
                Ok(v) => {
                    let room = v["room"].as_str().unwrap_or("").to_string();
                    let now = unix();
                    this.ring.book.dials.insert(room.clone(), Dial { room: room.clone(), peer, name: name.clone(), started: now, until: now + ring::DIRECT_WAIT });
                    this.ring.joining.push((room, name.clone(), now + 10.));
                    this.say(if v["invited"].as_bool() == Some(true) { format!("Calling {name}. This only works if they have a newer mega open; it ends in 30 seconds if nobody answers.") } else { format!("Calling {name}…") }, cx);
                }
                Err(e) => this.say(e, cx),
            }
        });
    }
    /// A direct call's room is in the snapshot: join it.
    fn dials_ready(&mut self, now: f64, window: &mut Window, cx: &mut Context<Self>) {
        if self.ring.joining.is_empty() { return; }
        let list = std::mem::take(&mut self.ring.joining);
        for (room, name, give_up) in list {
            if self.channel(&room).is_some() { self.join_named(room, name, window, cx); }
            else if now >= give_up { self.ring.book.dials.remove(&room); self.say("The call room is not ready yet. Try again in a moment.", cx); }
            else { self.ring.joining.push((room, name, give_up)); }
        }
    }
    /// The other person of the direct conversation that has this room.
    pub fn dm_peer(&self, gid: &str) -> Option<(String, String)> {
        self.channel(gid).filter(|(c, _)| c.direct && !c.peer.is_empty()).map(|(c, _)| (c.peer.clone(), c.name.clone()))
    }

    // ---- drawing ----
    /// The ring button in the call bar (only in a voice room's call).
    pub fn ring_button(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.ring_all_room()?;
        let on = self.ring.book.outs.contains_key("*");
        Some(div().id("c-ring").px_3().py_1().rounded_full().border_1().cursor_pointer().border_color(if on { look::paper() } else { look::line() })
            .when(on, |d| d.bg(look::paper()).text_color(rgb(0x0c0d12)))
            .child(if on { "Stop ringing" } else { "Ring everyone" })
            .on_click(cx.listener(|this, _, window, cx| this.ring_all(window, cx))).into_any_element())
    }
    /// "Call" for the header of a direct conversation.
    pub fn call_person_button(&mut self, gid: &str, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (peer, name) = self.dm_peer(gid)?;
        let busy = self.call.in_call() || !self.ring.joining.is_empty();
        Some(Button::new("dm-call").label(format!("Call {name}")).when(busy, |b| b.opacity(0.5))
            .on_click(cx.listener(move |this, _, window, cx| this.dial(peer.clone(), name.clone(), window, cx))).into_any_element())
    }

    /// The cards, the missed-call notes and the short notices, over the top right of the window.
    pub fn ring_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let cards: Vec<ring::Card> = self.ring.book.shown().to_vec();
        let missed = self.ring.book.missed.clone();
        let notes: Vec<String> = self.ring.notes.iter().map(|(t, _)| t.clone()).collect();
        if cards.is_empty() && missed.is_empty() && notes.is_empty() { return None; }
        let mut col = v_flex().id("ring-overlay").absolute().top(px(14.)).right(px(14.)).w(px(380.)).gap_2().occlude();
        for c in cards {
            let duo = self.room_duo(&c.gid);
            let ring = c.ring.clone();
            let (r2, r3) = (ring.clone(), ring.clone());
            let what = if c.direct || c.one_to_one { "calling you".to_string() } else { "ringing the room".to_string() };
            let where_ = if c.one_to_one { "direct call".to_string() } else { format!("{} · {}", c.room, c.comm) };
            col = col.child(v_flex().id(SharedString::from(format!("ring-{ring}"))).gap_1().p_4().rounded(px(22.)).bg(duo.field()).text_color(duo.ink()).border_1().border_color(look::line()).shadow_lg()
                .child(div().font_family(look::F_NAME).text_size(px(12.)).child(what))
                .child(div().font_family(look::F_SHARP).text_size(px(30.)).line_height(px(30.)).child(c.name.clone()))
                .child(div().font_family(look::F_NAME).text_size(px(12.)).child(where_))
                .child(h_flex().mt_2().gap_2().justify_end()
                    .child(Button::new(SharedString::from(format!("ring-no-{r2}"))).label("Decline").on_click(cx.listener(move |this, _, _, cx| this.ring_decline(&r2, cx))))
                    .child(Button::new(SharedString::from(format!("ring-yes-{r3}"))).primary().label("Join").on_click(cx.listener(move |this, _, window, cx| this.ring_accept(&r3, window, cx))))));
        }
        if !missed.is_empty() {
            let mut m = v_flex().id("missed").gap_1().px_4().py_2().rounded(px(18.)).bg(rgb(look::SLIP)).border_1().border_color(look::line()).shadow_lg();
            for x in &missed { m = m.child(div().font_family(look::F_NAME).text_size(px(13.)).child(format!("{} · {}", x.line(), crate::app::clock(x.at as u64)))); }
            m = m.child(h_flex().justify_end().child(div().id("missed-x").cursor_pointer().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child("dismiss")
                .on_click(cx.listener(|this, _, _, cx| { this.ring.book.dismiss_missed(); cx.notify(); }))));
            col = col.child(m);
        }
        for n in notes { col = col.child(div().px_4().py_2().rounded(px(18.)).bg(rgb(look::SLIP)).border_1().border_color(look::line()).shadow_lg().font_family(look::F_NAME).text_size(px(13.)).child(n)); }
        Some(col.into_any_element())
    }

    /// For checks: what the ring side of the window holds.
    pub fn ring_dump(&self) -> Value {
        let b = &self.ring.book;
        json!({
            "cards": b.cards.iter().map(|c| json!({"ring": c.ring, "gid": c.gid, "name": c.name, "room": c.room, "direct": c.direct, "one_to_one": c.one_to_one, "left": (c.until - unix()).round()})).collect::<Vec<_>>(),
            "missed": b.missed.iter().map(|m| m.line()).collect::<Vec<_>>(),
            "outs": b.outs.keys().collect::<Vec<_>>(), "dials": b.dials.keys().collect::<Vec<_>>(),
            "notes": self.ring.notes.iter().map(|(t, _)| t).collect::<Vec<_>>(),
            "stats": {"shown": self.ring.stats.shown, "tones": self.ring.stats.tones, "polls": self.ring.stats.polls},
            "since": b.since,
        })
    }
}

