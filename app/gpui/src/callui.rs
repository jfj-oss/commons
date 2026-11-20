//! The call's window parts: the call bar (who is here, speaking, mute, deafen, push-to-talk, leave), the settings panel
//! (devices, the three processing switches, push-to-talk, a person's volume) and the column of cameras and screens.
//! The engine (calls/engine.rs) does the calling and keeps every choice; this file only draws its snapshot and sends it
//! commands. Nothing here polls on a timer: the engine's change channel wakes the window.
use crate::app::{Level, Mega};
use crate::calls::engine::{self, AudioMode, CallEngine, Command, CoreLink, Options};
use crate::calls::settings::CallSettings;
use crate::deck::{self, Deck};
use crate::look;
use crate::video::{self, Frames, Kind, Shown, Tile};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct Linked(pub Arc<crate::core::Core>);
impl CoreLink for Linked {
    fn call(&self, method: &str, args: Value) -> Result<Value, String> { self.0.call(method, args) }
    fn call_active(&self) -> bool { self.0.app.call_active() }
}

/// What is being dragged: one tile of the column.
#[derive(Clone)]
pub struct TileDrag { pub sid: String, pub label: String }
struct Ghost(String);
impl Render for Ghost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().px_3().py_1().rounded_full().bg(rgb(look::SLIP)).border_1().border_color(look::paper()).font_family(look::F_NAME).text_size(px(13.)).child(self.0.clone())
    }
}

pub struct CallUi {
    pub engine: Option<Arc<CallEngine>>,
    pub snap: engine::Snapshot,
    pub settings_open: bool,
    /// The person whose menu is open (their account), with the slider that sets their volume.
    pub menu: Option<(String, String)>,
    pub vol: Entity<SliderState>,
    pub frames: Arc<Frames>,
    pub shown: Shown,
    pub fake_alive: Arc<AtomicBool>,
    pub tiles: Vec<Tile>,
    pub deck: Deck,
    /// The column's own width (0 = the default), kept per device in the prefs.
    pub deck_w: f32,
    pub resize: Option<(f32, f32)>,
    pub grip: FocusHandle,
    pub live_since: u64,
    pub live_ready: bool,
    pub live_busy: bool,
    pub asked: bool,
    pub ptt_key: String,
    /// The call's pictures have their own window (the main window shows no column meanwhile).
    pub popped: bool,
    pub mini: Option<AnyWindowHandle>,
    pub pop_handle: Option<AnyWindowHandle>,
    pub error_shown: Option<String>,
    /// Where each control was last painted, for checks.
    pub hits: crate::calldock::Hits,
    /// The lock's explainer is open.
    pub explain: bool,
    /// The rune card (song, controls, listen along) is open beside the dock: opened by the ♪ button, not always shown.
    pub rune_open: bool,
    /// The error notice the person closed.
    pub err_hidden: Option<String>,
    /// The settings drop-down that is open (its id).
    pub dd: Option<&'static str>,
    /// How a screen is shared next (the page's "Share as").
    pub share_profile: crate::calls::share::Profile,
    /// The column of cameras and screens is hidden for me (the page's `hide`).
    pub deck_hidden: bool,
    pub _tasks: Vec<Task<()>>,
    pub _subs: Vec<Subscription>,
}

impl CallUi {
    pub fn new(deck_w: f32, window: &mut Window, cx: &mut Context<Mega>) -> CallUi {
        let vol = cx.new(|_| SliderState::new().min(0.).max(3.).step(0.05).default_value(1.));
        let (frames, mut rx) = Frames::new();
        let sub = cx.subscribe_in(&vol, window, |this: &mut Mega, _, ev: &SliderEvent, _, cx| {
            if let SliderEvent::Change(v) = ev { this.set_person_volume(v.end() as f64, cx); }
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            use futures::StreamExt;
            while rx.next().await.is_some() {
                if this.update(cx, |this, cx| { this.call.frames.ack(); cx.notify(); }).is_err() { break; }
            }
        });
        CallUi {
            engine: None, snap: Default::default(), settings_open: false, menu: None, vol, frames, shown: Shown::default(), fake_alive: Arc::new(AtomicBool::new(false)), tiles: vec![], deck: Deck::default(),
            deck_w, resize: None, grip: cx.focus_handle(), live_since: 0, live_ready: false, live_busy: false, asked: false, ptt_key: "`".into(), popped: false, mini: None, pop_handle: None, error_shown: None, hits: Default::default(), explain: false, rune_open: false, err_hidden: None, dd: None, share_profile: crate::calls::share::Profile::Text, deck_hidden: false,
            _tasks: vec![task], _subs: vec![sub],
        }
    }
    pub fn in_call(&self) -> bool { matches!(self.snap.phase.as_str(), "connecting" | "connected" | "reconnecting") }
    pub fn connected(&self) -> bool { matches!(self.snap.phase.as_str(), "connected" | "reconnecting") }
    /// Accounts of the other people in the call (members only, as mega-deck.js `others`).
    pub fn others(&self) -> HashSet<String> { self.snap.peers.iter().filter(|p| !p.me && p.member).filter_map(|p| p.account.clone()).collect() }
    pub fn me_account(&self) -> String { self.snap.peers.iter().find(|p| p.me).and_then(|p| p.account.clone()).unwrap_or_default() }
    pub fn tile_ids(&self) -> Vec<String> { self.tiles.iter().map(|t| t.sid.clone()).collect() }
    pub fn tile(&self, sid: &str) -> Option<&Tile> { self.tiles.iter().find(|t| t.sid == sid) }
    pub fn ordered(&self) -> Vec<Tile> { self.deck.order.iter().filter_map(|s| self.tile(s).cloned()).collect() }
}

fn pill(id: impl Into<ElementId>, on: bool, text: impl Into<SharedString>) -> Stateful<Div> {
    div().id(id).px_3().py_1().rounded_full().border_1().cursor_pointer().font_family(look::F_NAME).text_size(px(13.)).border_color(if on { look::paper() } else { look::line() })
        .when(on, |d| d.bg(look::paper()).text_color(rgb(0x0c0d12))).child(text.into())
}

impl Mega {
    // ---- the engine ----
    pub fn engine(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Arc<CallEngine> {
        if let Some(e) = &self.call.engine { return e.clone(); }
        let settings_path = self.core.dir.join("gpui-call-prefs.json");
        let settings = CallSettings::load(&settings_path);
        let e = Arc::new(CallEngine::start(Arc::new(Linked(self.core.clone())), Options { audio: AudioMode::from_env(), settings, settings_path: Some(settings_path), shortcuts: crate::calls::shortcuts::Mode::from_env(), frames: Some(self.call.frames.clone()) }));
        // the engine's change channel wakes the window: nothing else does, so a call that changes nothing costs nothing
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        let mut ch = e.changes();
        e.rt.spawn(async move { while ch.changed().await.is_ok() { if tx.unbounded_send(()).is_err() { break; } } });
        self.call._tasks.push(cx.spawn_in(window, async move |this, cx| {
            use futures::StreamExt;
            while rx.next().await.is_some() { if this.update_in(cx, |this, window, cx| this.engine_changed(window, cx)).is_err() { break; } }
        }));
        self.call.engine = Some(e.clone());
        e
    }

    pub fn join_call(&mut self, gid: String, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.channel(&gid).map(|(_, ch)| ch.name.clone()).unwrap_or_default();
        let e = self.engine(window, cx);
        e.send(Command::Join { gid, name });
        self.call.snap = e.snapshot();
        cx.notify();
    }
    pub fn leave_call(&mut self, cx: &mut Context<Self>) {
        if let Some(e) = &self.call.engine { e.send(Command::Leave); }
        self.set_settings_open(false); self.call.menu = None;
        cx.notify();
    }
    pub fn set_settings_open(&mut self, on: bool) {
        self.call.settings_open = on;
        if !on { self.call_cmd(Command::MicMeter(false)); }
    }
    pub fn call_cmd(&mut self, c: Command) { if let Some(e) = &self.call.engine { e.send(c); } }

    fn engine_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(e) = self.call.engine.clone() else { return };
        let was_in = self.call.in_call();
        self.call.snap = e.snapshot();
        let in_call = self.call.in_call();
        if was_in && !in_call { self.call_ended(window); }
        // the column's tiles: the engine's subscribed cameras and screens (and the synthetic ones when asked for)
        let snap = self.call.snap.clone();
        // others' cameras and screens, then your own (the preview of what you send); a camera that is off has no tile
        let mut all = snap.subscribed.clone();
        all.extend(snap.local_video.iter().cloned());
        let mut tiles = video::tiles_from(&all, |id| snap.peers.iter().find(|p| p.identity == id).map(|p| if p.me { "You".to_string() } else { p.name.clone() }).unwrap_or_else(|| "someone".into()));
        tiles.retain(|t| !snap.muted_video.iter().any(|m| m.ends_with(&format!("/{}", t.sid))));
        let fake = video::fake_count();
        if fake > 0 && self.call.connected() {
            if !self.call.fake_alive.swap(true, Ordering::SeqCst) { video::start_fake(self.call.frames.clone(), fake, self.call.fake_alive.clone()); }
            let others: Vec<_> = snap.peers.iter().filter(|p| !p.me).collect();
            for i in 0..fake {
                let who = others.get(i % others.len().max(1)).map(|p| (p.identity.clone(), p.name.clone())).unwrap_or(("me".into(), "You".into()));
                tiles.push(Tile { sid: format!("fake{i}"), identity: who.0, name: who.1, kind: if i % 3 == 1 { Kind::Screen } else { Kind::Camera } });
            }
        } else if self.call.fake_alive.swap(false, Ordering::SeqCst) { /* feeders stop themselves */ }
        if tiles != self.call.tiles {
            self.call.tiles = tiles;
            let ids = self.call.tile_ids();
            self.call.deck.layout(&ids);
            self.call.shown.keep_only(&ids, window);
        }
        if let Some(err) = &snap.error { if self.call.error_shown.as_deref() != Some(err.msg.as_str()) { self.call.error_shown = Some(err.msg.clone()); } }
        cx.notify();
    }
    fn call_ended(&mut self, window: &mut Window) {
        self.call.tiles.clear();
        self.call.deck.reset();
        self.call.asked = false;
        self.call.fake_alive.store(false, Ordering::SeqCst);
        self.call.shown.keep_only(&[], window);
        self.call.popped = false;
        self.call.pop_handle = None;
    }

    // ---- who is in the call, and the live channel (the order of the tiles) ----
    pub fn call_gid(&self) -> Option<String> { if self.call.connected() { self.call.snap.gid.clone() } else { None } }
    pub fn live_send(&mut self, kind: &'static str, data: Value, window: &mut Window, cx: &mut Context<Self>) {
        let Some(gid) = self.call_gid() else { return };
        let core = self.core.clone();
        let _ = window;
        cx.background_spawn(async move { let _ = core.call("live_send", json!({"gid": gid, "kind": kind, "data": data.to_string()})); }).detach();
    }
    /// Every 700 ms while others are in the call: read their `deck.*` events, and ask once for the order.
    pub fn call_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.call.popped || self.call.mini.is_some() { self.pop_windows_check(cx); }
        let Some(gid) = self.call_gid() else { self.call.live_ready = false; return };
        let others = self.call.others();
        if others.is_empty() { return; }
        if self.call.deck.should_ask(&gid, true) && !self.call.asked { self.call.asked = true; self.live_send("deck.q", json!({}), window, cx); }
        if self.call.live_busy { return; }
        self.call.live_busy = true;
        let since = self.call.live_since;
        self.rpc("live_poll", json!({"since": since, "gid": gid.clone()}), window, cx, move |this, r, window, cx| {
            this.call.live_busy = false;
            let Ok(r) = r else { return };
            let head = r["head"].as_u64().unwrap_or(0);
            if !std::mem::replace(&mut this.call.live_ready, true) { this.call.live_since = this.call.live_since.max(head); return; }
            let events = r["events"].as_array().cloned().unwrap_or_default();
            for e in &events { if let Some(id) = e["id"].as_u64() { this.call.live_since = this.call.live_since.max(id); } this.deck_event(e, window, cx); }
        });
    }
    pub fn deck_event(&mut self, e: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let (kind, acct) = (e["kind"].as_str().unwrap_or(""), e["account"].as_str().unwrap_or(""));
        if kind != "deck.m" && kind != "deck.q" { return; }
        if self.call_gid().as_deref() != e["gid"].as_str() { return; }
        let who = self.call.others();
        if !who.contains(acct) { return; }
        if kind == "deck.q" {
            if self.call.deck.should_answer() { let p = self.call.deck.payload("", true, true); self.live_send("deck.m", p, window, cx); }
            return;
        }
        let Ok(d) = serde_json::from_str::<Value>(e["data"].as_str().unwrap_or("")) else { return };
        let (me, known) = (self.call.me_account(), self.call.tile_ids());
        if self.call.deck.on_move(acct, &d, &me, &who, &known, Instant::now()) { cx.notify(); }
    }
    /// A tile is dragged over another's place (or dropped there).
    pub fn tile_to(&mut self, sid: &str, to: usize, end: bool, window: &mut Window, cx: &mut Context<Self>) {
        let me = self.call.me_account();
        let others = !self.call.others().is_empty();
        if self.call.deck.drag.as_deref() != Some(sid) { self.call.deck.grab(sid, &me, Instant::now()); }
        if let Some(p) = self.call.deck.moved(sid, to, &me, others, end, Instant::now()) { self.live_send("deck.m", p, window, cx); }
        if end { self.call.deck.release(sid); }
        cx.notify();
    }

    // ---- choices ----
    pub fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.call.settings_open = !self.call.settings_open;
        if self.call.settings_open { let e = self.engine(window, cx); e.send(Command::ListDevices); e.send(Command::ListCameras); e.send(Command::MicMeter(true)); self.call.snap = e.snapshot(); }
        else { self.call_cmd(Command::MicMeter(false)); }
        cx.notify();
    }
    pub fn open_person(&mut self, account: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.call.menu.as_ref().map(|m| &m.0) == Some(&account) { self.call.menu = None; cx.notify(); return; }
        let v = self.call.snap.peers.iter().find(|p| p.account.as_deref() == Some(account.as_str())).map(|p| p.volume).unwrap_or(1.);
        self.call.vol.update(cx, |s, cx| s.set_value(v.clamp(0., 3.) as f32, window, cx));
        self.call.menu = Some((account, name));
        cx.notify();
    }
    fn set_person_volume(&mut self, v: f64, cx: &mut Context<Self>) {
        let Some((acct, _)) = self.call.menu.clone() else { return };
        self.call_cmd(Command::SetVolume { account: acct, volume: v });
        cx.notify();
    }
    pub fn set_deck_width(&mut self, w: f32, window: &Window, save: bool) {
        // 0 means "the default for this window" (a reset), kept as 0 so the default follows the window's size
        self.call.deck_w = if w <= 0. { 0. } else { deck::clamp_width(w, f32::from(window.viewport_size().width)) };
        if save { self.prefs.deck_w = self.call.deck_w; self.prefs.save(); }
    }
    pub fn deck_width(&self, window: &Window) -> f32 { deck::clamp_width(self.call.deck_w, f32::from(window.viewport_size().width)) }

    // ---- drawing ----
    /// One person's volume, beside the call card (the page's `.call-pop.vol`).
    pub fn call_popover(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let snap = self.call.snap.clone();
        let (acct, name) = self.call.menu.clone()?;
        let v = snap.peers.iter().find(|p| p.account.as_deref() == Some(acct.as_str())).map(|p| p.volume).unwrap_or(1.);
        let a2 = acct.clone();
        let hits = self.call.hits.clone();
        Some(v_flex().id("person-menu").absolute().right(px(crate::calldock::DOCK_W + 24.)).bottom(px(24.)).w(px(260.)).gap_2().p_4().rounded(px(18.)).bg(rgb(0x14131a)).border_1().border_color(look::line()).shadow_lg().occlude()
            .child(div().font_family(look::F_UI).font_weight(FontWeight::SEMIBOLD).text_size(px(14.)).child(name))
            .child(div().font_family(look::F_UI).text_size(px(12.)).text_color(look::mute()).child(format!("Volume for you only: {}%", (v * 100.).round() as i64)))
            .child(div().relative().child(Slider::new(&self.call.vol).horizontal()).child(hits.track("v-slider")))
            .child(h_flex().gap_2()
                .child(pill("v-0", v == 0., if v == 0. { "Unmute for me" } else { "Mute for me" }).relative().child(hits.track("v-0")).on_click(cx.listener(move |this, _, window, cx| {
                    let to = if v == 0. { 1. } else { 0. };
                    this.call_cmd(Command::SetVolume { account: acct.clone(), volume: to }); this.call.vol.update(cx, |s, cx| s.set_value(to as f32, window, cx)); cx.notify(); })))
                .child(pill("v-1", (v - 1.).abs() < 0.001, "Normal").relative().child(hits.track("v-1")).on_click(cx.listener(move |this, _, window, cx| { this.call_cmd(Command::SetVolume { account: a2.clone(), volume: 1. }); this.call.vol.update(cx, |s, cx| s.set_value(1., window, cx)); cx.notify(); })))
                .child(div().flex_1())
                .child(pill("v-x", false, "Close").relative().child(hits.track("v-x")).on_click(cx.listener(|this, _, _, cx| { this.call.menu = None; cx.notify(); })))).into_any_element())
    }

    /// The share button: end the share that runs, or start one (the system's picker opens; a test run uses a synthetic screen).
    pub fn toggle_share(&mut self) {
        if matches!(self.call.snap.share.state.as_str(), "" | "idle") {
            let source = if crate::video::test_sources() { crate::calls::capture::ShareSource::Test(Default::default()) } else { crate::calls::capture::ShareSource::Portal };
            self.call_cmd(Command::StartShare { profile: self.call.share_profile, source, audio: None });
        } else { self.call_cmd(Command::StopShare); }
    }
    pub fn toggle_camera(&mut self) {
        if !matches!(self.call.snap.camera.state.as_str(), "" | "off") { self.call_cmd(Command::StopCamera); return; }
        let source = if crate::video::test_sources() { crate::calls::camera::CameraSource::Test } else { crate::calls::camera::CameraSource::Device(None) };
        self.call_cmd(Command::StartCamera { source, profile: None });
    }

    // ---- the column of cameras and screens ----
    fn tile_view(&mut self, t: &Tile, ix: usize, cx: &mut Context<Self>, window: &mut Window) -> impl IntoElement {
        let pic = self.call.shown.take(&self.call.frames, &t.sid, window);
                let me = self.call.me_account();
        let who = self.call.others();
        let holder = self.call.deck.holder(&t.sid, &me, &who, Instant::now()).and_then(|a| self.call.snap.peers.iter().find(|p| p.account.as_deref() == Some(a.as_str())).map(|p| p.name.clone()));
        let (sid, sid2, sid3, label) = (t.sid.clone(), t.sid.clone(), t.sid.clone(), format!("{} · {}", t.name, if t.kind == Kind::Screen { "screen" } else { "camera" }));
        let held_by_other = holder.is_some();
        let mut d = div().id(SharedString::from(format!("tile-{}", t.sid))).relative().w_full().flex_none().rounded(px(14.)).overflow_hidden().bg(rgb(0x0b0a10)).border_2().cursor_grab()
            .border_color(hsla(0., 0., 0., 0.))
            .when(held_by_other, |d| d.opacity(0.6))
            .on_drag(TileDrag { sid: t.sid.clone(), label: label.clone() }, |v, _, _, cx| cx.new(|_| Ghost(v.label.clone())))
            .on_drag_move::<TileDrag>(cx.listener(move |this, ev: &DragMoveEvent<TileDrag>, window, cx| {
                if ev.bounds.contains(&ev.event.position) { let from = ev.drag(cx).sid.clone(); if from != sid { this.tile_to(&from, ix, false, window, cx); } }
            }))
            .on_drop::<TileDrag>(cx.listener(move |this, v: &TileDrag, window, cx| this.tile_to(&v.sid, ix, true, window, cx)));
        // 16:9 by padding: the height follows the width
        d = d.child(div().w_full().pb(relative(0.5625)));
        let inner = div().absolute().inset_0();
        let inner = match pic {
            Some(i) => inner.child(img(ImageSource::Render(i)).size_full().object_fit(ObjectFit::Cover)),
            None => {
                let acct = self.call.snap.peers.iter().find(|p| p.identity == t.identity).and_then(|p| p.account.clone()).unwrap_or_default();
                match crate::avatars::get(&acct) {
                    // no camera: their picture, centred, as the page's `.cp img` stands for them
                    Some(p) => inner.bg(look::face(&t.name).opacity(0.25)).flex().items_center().justify_center()
                        .child(div().h(relative(0.72)).aspect_square().child(img(ImageSource::Render(p.at(200.))).size_full().object_fit(ObjectFit::Fill))),
                    None => inner.bg(look::face(&t.name).opacity(0.25)).flex().items_center().justify_center().child(div().font_family(look::F_SHARP).text_size(px(54.)).text_color(look::face(&t.name)).child(t.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default())),
                }
            }
        };
        let hits = self.call.hits.clone();
        d.child(inner)
            .child(h_flex().absolute().left_0().right_0().bottom_0().gap_1().items_center().px(px(8.)).py(px(4.)).bg(linear_gradient(180., linear_color_stop(hsla(0., 0., 0., 0.), 0.), linear_color_stop(hsla(0., 0., 0., 0.7), 1.)))
                .font_family(look::F_UI).text_size(px(11.)).text_color(look::paper())
                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(label.clone()))
                .when_some(holder, |d, h| d.child(div().text_color(look::mute()).child(format!("{h} is moving it"))))
                .child(div().id(SharedString::from(format!("tile-pop-{}", t.sid))).relative().px(px(8.)).py(px(2.)).rounded(px(6.)).bg(hsla(0., 0., 1., 0.18)).cursor_pointer().child("pop out").child(hits.track(format!("tile-pop-{}", t.sid)))
                    .on_click(cx.listener(move |this, _, window, cx| { cx.stop_propagation(); this.pop_tile(sid2.clone(), window, cx); }))))
            .child(hits.track(format!("tile-{}", t.sid)))
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| { if ev.click_count() >= 2 { this.pop_tile(sid3.clone(), window, cx); } }))
    }

    /// The column and the grip on its edge; `None` while there is nothing to show.
    pub fn deck_column(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.call.tiles.is_empty() || !self.call.connected() || self.call.popped { self.call.deck_hidden = false; return None; }
        if self.call.deck_hidden { return None; }
        let w = self.deck_width(window);
        let order = self.call.ordered();
        let vw = f32::from(window.viewport_size().width);
        let (lo, hi, _) = deck::limits(vw);
        let mut col = v_flex().id("deck").flex_none().w(px(w)).h_full().overflow_y_scroll().gap_3().px_3().pt_4().pb_3().bg(rgb(look::VOID));
        let hits = self.call.hits.clone();
        col = col.child(h_flex().flex_none().items_center().gap_2().min_h(px(28.)).font_family(look::F_UI).text_size(px(12.))
            .child(div().flex_1().min_w_0().opacity(0.75).child("cameras and screens"))
            .child(div().id("deck-hide").relative().px(px(12.)).py(px(6.)).rounded_full().bg(hsla(0., 0., 1., 0.12)).cursor_pointer().child("hide").child(hits.track("deck-hide"))
                .on_click(cx.listener(|this, _, _, cx| { this.call.deck_hidden = true; cx.notify(); }))));
        for (i, t) in order.iter().enumerate() { col = col.child(self.tile_view(t, i, cx, window)); }
        let active = self.call.resize.is_some();
        let grip = div().id("deck-grip").relative().child(self.call.hits.track("deck-grip")).track_focus(&self.call.grip).flex_none().w(px(9.)).h_full().cursor_col_resize().flex().justify_center()
            .child(div().w(px(if active { 3. } else { 1. })).h_full().bg(if active { look::paper() } else { look::line() }))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                window.focus(&this.call.grip.clone(), cx);
                if ev.click_count >= 2 { this.set_deck_width(0., window, true); this.call.resize = None; } else { let w = this.deck_width(window); this.call.resize = Some((f32::from(ev.position.x), w)); }
                cx.notify();
            }))
            .on_key_down(cx.listener(move |this, ev: &KeyDownEvent, window, cx| {
                let k = ev.keystroke.key.as_str();
                let vw = f32::from(window.viewport_size().width);
                let w = match k {
                    "right" => deck::arrow(this.deck_width(window), vw, 1., ev.keystroke.modifiers.shift),
                    "left" => deck::arrow(this.deck_width(window), vw, -1., ev.keystroke.modifiers.shift),
                    "home" => deck::limits(vw).0,
                    "end" => deck::limits(vw).1,
                    _ => return,
                };
                this.set_deck_width(w, window, true);
                cx.stop_propagation(); cx.notify();
            }));
        let _ = (lo, hi);
        Some(h_flex().h_full().flex_none().child(col).child(grip).into_any_element())
    }

    /// "show N tiles": where the column was hidden (the page's `#mega-deckshow`).
    pub fn deck_show_button(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.call.deck_hidden || self.call.tiles.is_empty() || !self.call.connected() { return None; }
        let n = self.call.tiles.len();
        let hits = self.call.hits.clone();
        Some(div().id("deck-show").absolute().left(px(10.)).top(px(10.)).px(px(12.)).py(px(6.)).rounded_full().bg(rgb(0x2a2830)).cursor_pointer().font_family(look::F_UI).text_size(px(12.)).child(format!("show {n} {}", if n == 1 { "tile" } else { "tiles" })).child(hits.track("deck-show"))
            .on_click(cx.listener(|this, _, _, cx| { this.call.deck_hidden = false; cx.notify(); })).into_any_element())
    }

    /// Dragging the edge: called for mouse moves and releases on the window's root.
    pub fn resize_move(&mut self, x: f32, window: &Window, cx: &mut Context<Self>) {
        let Some((x0, w0)) = self.call.resize else { return };
        self.set_deck_width(w0 + (x - x0), window, false);
        cx.notify();
    }
    pub fn resize_end(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.call.resize.take().is_some() { self.set_deck_width(self.call.deck_w, window, true); cx.notify(); }
    }

    /// Push-to-talk keys: the key goes to the call, never into the composer.
    pub fn ptt_key_event(&mut self, key: &str, down: bool, cx: &mut Context<Self>) -> bool {
        if !self.call.snap.push_to_talk || !self.call.in_call() || key != self.call.ptt_key { return false; }
        if self.call.snap.ptt_held == down && down { return true; }
        self.call_cmd(Command::PushToTalk(down));
        cx.stop_propagation();
        true
    }
}


impl Mega {
    /// For checks: the engine's state, the window's call state, and where every control was painted.
    pub fn callsnap(&self) -> Value {
        let c = &self.call; let s = &c.snap;
        json!({
            "phase": s.phase, "gid": s.gid, "room": s.room_name, "muted": s.muted, "deafened": s.deafened, "pushToTalk": s.push_to_talk, "pttHeld": s.ptt_held,
            "echo": s.echo_cancellation, "noise": s.noise_suppression, "agc": s.auto_gain_control,
            "camera": s.camera.state, "camProfile": s.camera.profile, "share": s.share.state, "shareProfile": s.share.profile,
            "peers": s.peers.iter().map(|p| json!({"identity": p.identity, "name": p.name, "me": p.me, "speaking": p.speaking, "muted": p.muted, "deafened": p.deafened, "volume": p.volume, "account": p.account})).collect::<Vec<_>>(),
            "subscribed": s.subscribed, "localVideo": s.local_video, "micId": s.mic_id, "outId": s.out_id,
            "ui": {"settings": c.settings_open, "menu": c.menu.as_ref().map(|m| m.1.clone()), "popped": c.popped, "mini": c.mini.is_some(), "explain": c.explain, "dd": c.dd, "deckW": self.deck_w_now(), "deckHidden": c.deck_hidden,
                   "tiles": c.tile_ids(), "order": c.deck.order, "shareProfile": c.share_profile.name(), "docked": self.dock_on()},
            "ring": self.ring_dump(), "rune": self.rune_dump(),
            "hits": c.hits.dump(),
        })
    }
    pub fn deck_w_now(&self) -> f32 { self.call.deck_w }
}

// ---- automation for checks: MEGA_GPUI_UI_STEPS="join;wait=4000;settings;wait=1000;..." ----
impl Mega {
    /// Runs the steps of `MEGA_GPUI_UI_STEPS` once, a little after sign-in, so a screenshot run needs no pointer.
    pub fn start_ui_steps(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(steps) = std::env::var("MEGA_GPUI_UI_STEPS") else { return };
        static ONCE: AtomicBool = AtomicBool::new(false);
        if ONCE.swap(true, Ordering::SeqCst) { return; }
        let steps: Vec<String> = steps.split(';').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        let task = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(2500)).await;
            for s in steps {
                if let Some(ms) = s.strip_prefix("wait=") { cx.background_executor().timer(Duration::from_millis(ms.parse().unwrap_or(500))).await; continue; }
                let ok = this.update_in(cx, |this, window, cx| this.ui_step(&s, window, cx)).is_ok();
                if !ok { break; }
                cx.background_executor().timer(Duration::from_millis(400)).await;
            }
        });
        self.call._tasks.push(task);
    }
    fn ui_step(&mut self, s: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (k, v) = s.split_once('=').unwrap_or((s, ""));
        match k {
            "join" => { if let Some(g) = self.gid.clone() { self.join_call(g, window, cx); } }
            "leave" => self.leave_call(cx),
            "settings" => self.open_settings(window, cx),
            "person" => { let p = self.call.snap.peers.iter().find(|p| !p.me && p.account.is_some()).cloned(); if let Some(p) = p { self.open_person(p.account.unwrap(), p.name, window, cx); } }
            "close" => { self.call.menu = None; self.set_settings_open(false); cx.notify(); }
            "pop" => self.pop_call(window, cx),
            "poptile" => { if let Some(t) = self.call.tiles.first().cloned() { self.pop_tile(t.sid, window, cx); } }
            "mini" => self.toggle_mini(window, cx),
            "width" => { self.set_deck_width(v.parse().unwrap_or(0.), window, true); cx.notify(); }
            "move" => { if let Some((sid, ix)) = v.split_once(':') { self.tile_to(sid, ix.parse().unwrap_or(0), true, window, cx); } }
            "mute" => { let m = self.call.snap.muted; self.call_cmd(Command::SetMuted(!m)); }
            "ptt" => self.call_cmd(Command::SetPushToTalk(v != "off")),
            "pttdown" => self.call_cmd(Command::PushToTalk(true)),
            "pttup" => self.call_cmd(Command::PushToTalk(false)),
            "volume" => { if let Some(a) = self.call.snap.peers.iter().find(|p| !p.me).and_then(|p| p.account.clone()) { self.call_cmd(Command::SetVolume { account: a, volume: v.parse().unwrap_or(1.) }); } }
            "level" => { match v { "home" => self.go(Level::Home, window, cx), "settings" => self.go(Level::Settings, window, cx), _ => {} } }
            "colours" => { self.colours_open = !self.colours_open; cx.notify(); }
            "preset" => { let i: usize = v.parse().unwrap_or(99); let d = look::PRESETS.get(i).map(|p| (p.1, p.2)); self.set_room_colours(d, window, cx); }
            "dmunread" => { self.fake_dm_unread = v.parse().unwrap_or(0); cx.notify(); }
            "open" => { if let Some((cid, gid)) = self.snap.as_ref().and_then(|s| s.direct.iter().find(|d| d.name == v).map(|d| (d.id.clone(), d.room.clone()))) { self.open_room(cid, gid, window, cx); } }
            "stir" => { self.stir(2); cx.notify(); }
            "dump" => { let s = self.call.snap.clone(); eprintln!("UI-DUMP {}", serde_json::to_string(&json!({"snap": s, "tiles": self.call.tile_ids(), "order": self.call.deck.order, "w": self.deck_width(window), "popped": self.call.popped})).unwrap_or_default()); }
            _ => eprintln!("ui step? {s}"),
        }
    }
}

