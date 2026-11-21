//! First run: the screens before there is an account on this computer. Web reference: app.js `boot` (new account, a second device,
//! recovery), mega-invite.js `first` (the first screen: an invite password, a name, one password for this device).
//!   * Invite   an invite password (or a pasted invite or relay link), your name, a password for this device: `create_account_password`
//!              (a link in the same box goes the old way: `parse_link`, `create_account`, `join`)
//!   * Create   a new account, with an invite link if there is one
//!   * Pair     "this is a new device for my account": `start_pairing`, then the code, its QR and the check digits wait for an
//!              existing device to approve (`pairing_status` every 1.5 s, `pairing_abandon` to start over)
//!   * Recover  the recovery key and a new password: `recover_account`
//! Everything that touches keys or the relay is the core's; this file only asks and shows. The recovery key itself (shown once after
//! a new account, kept in a corner card until it is saved) is in devices.rs.
use crate::app::{Mega, Screen};
use crate::look;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex, Disableable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Boot { Invite, Create, Pair, Recover }

/// What a pasted invite or relay link said (`parse_link`).
#[derive(Clone, Debug, Default)]
pub struct Invited { pub kind: String, pub relay: String, pub secret: String, pub name: String, pub link: String }

/// A pairing that waits for approval: the code, the digits both devices must show, and the QR as filled squares in its own units.
#[derive(Clone, Debug, Default)]
pub struct PairInfo { pub code: String, pub check: String, pub dim: f32, pub squares: Vec<[f32; 4]> }

pub struct Onboard {
    pub mode: Boot,
    pub ipw: Entity<InputState>,
    pub link: Entity<InputState>,
    pub name: Entity<InputState>,
    pub pp: Entity<InputState>,
    pub pp2: Entity<InputState>,
    pub relay: Entity<InputState>,
    pub secret: Entity<InputState>,
    pub rk: Entity<InputState>,
    pub relay_open: bool,
    pub hint: String,
    pub link_note: String,
    pub invited: Option<Invited>,
    pub pair: Option<PairInfo>,
    pair_asked: Option<Instant>,
    /// The device password just typed, kept in memory only until the recovery key has been shown once (mega-invite.js `kept`).
    pub kept: String,
    pub busy: bool,
    pub err: Option<String>,
    link_seq: u64,
}

const HINT0: &str = "It locks mega on this computer. Nobody can reset it, so make it one you will remember.";

/// app.js `ppStrength`.
pub fn pp_strength(p: &str) -> String {
    if p.is_empty() { return String::new(); }
    if p.chars().count() < 8 { return "Too short: at least 8 characters.".into(); }
    let kinds = [p.chars().any(|c| c.is_ascii_lowercase()), p.chars().any(|c| c.is_ascii_uppercase()), p.chars().any(|c| c.is_ascii_digit()), p.chars().any(|c| !c.is_ascii_alphanumeric())].iter().filter(|b| **b).count();
    let words = p.split_whitespace().count();
    let score = p.chars().count() + kinds * 2 + if words >= 4 { 8 } else { 0 };
    (if score < 16 { "Weak. Someone with this disk could guess it. Four or more random words work well." } else if score < 24 { "Fair. A little longer would be better." } else { "Strong." }).into()
}

/// The core's QR (an SVG of `M<left> <top>h<w>v<h>H<left>V<top>` squares in a `viewBox`) as squares.
pub fn qr_squares(svg: &str) -> Option<(f32, Vec<[f32; 4]>)> {
    let vb = svg.split("viewBox=\"").nth(1)?.split('"').next()?;
    let mut it = vb.split_whitespace().map(|x| x.parse::<f32>().ok());
    let (_, _, w, _) = (it.next()??, it.next()??, it.next()??, it.next()??);
    let d = svg.split(" d=\"").nth(1)?.split('"').next()?;
    let mut out = vec![];
    for part in d.split('M').skip(1) {
        // "<left> <top>h<w>v<h>H<left>V<top>"
        let (xy, rest) = part.split_once('h')?;
        let (x, y) = xy.split_once(' ')?;
        let (ww, rest) = rest.split_once('v')?;
        let hh = rest.split('H').next()?;
        out.push([x.trim().parse().ok()?, y.trim().parse().ok()?, ww.trim().parse().ok()?, hh.trim().parse().ok()?]);
    }
    if out.is_empty() { None } else { Some((w, out)) }
}

pub fn is_link(v: &str) -> bool { let v = v.trim().to_lowercase(); v.starts_with("commons:") || v.starts_with("http:") || v.starts_with("https:") }

fn linkish(id: &'static str, text: &str) -> Stateful<Div> {
    div().id(id).font_family(look::F_NAME).text_size(px(13.)).text_color(look::mute()).cursor_pointer().underline().hover(|d| d.text_color(look::paper())).child(text.to_string())
}
fn lab(t: &str) -> Div { div().mt_3().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child(t.to_string()) }
fn small(t: impl Into<SharedString>) -> Div { div().text_size(px(13.)).text_color(look::mute()).child(t.into()) }

impl Onboard {
    pub fn new(window: &mut Window, cx: &mut Context<Mega>) -> (Onboard, Vec<Subscription>) {
        let ipw = cx.new(|cx| InputState::new(window, cx).placeholder("the password you were told"));
        let link = cx.new(|cx| InputState::new(window, cx).placeholder("Paste what your friend sent you"));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Your name"));
        let pp = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("A password for this device"));
        let pp2 = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("Type it again"));
        let relay = cx.new(|cx| InputState::new(window, cx).placeholder("https://chat.example.org"));
        let secret = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("Invite secret (only if this relay asks for one)"));
        let rk = cx.new(|cx| InputState::new(window, cx).placeholder("Your recovery key"));
        let mut subs = vec![];
        for e in [&ipw, &name, &pp, &pp2, &relay, &secret, &rk] {
            subs.push(cx.subscribe_in(e, window, |this: &mut Mega, _, ev: &InputEvent, window, cx| { if let InputEvent::PressEnter { .. } = ev { this.ob_go(window, cx); } }));
        }
        subs.push(cx.subscribe_in(&pp, window, |this: &mut Mega, _, ev: &InputEvent, _, cx| { if let InputEvent::Change = ev { this.ob.hint = pp_strength(this.ob.pp.read(cx).value().as_ref()); cx.notify(); } }));
        subs.push(cx.subscribe_in(&link, window, |this: &mut Mega, _, ev: &InputEvent, window, cx| {
            match ev { InputEvent::Change => this.ob_read_link(window, cx), InputEvent::PressEnter { .. } => this.ob_go(window, cx), _ => {} }
        }));
        (Onboard { mode: Boot::Invite, ipw, link, name, pp, pp2, relay, secret, rk, relay_open: false, hint: String::new(), link_note: String::new(), invited: None, pair: None, pair_asked: None, kept: String::new(), busy: false, err: None, link_seq: 0 }, subs)
    }
}

impl Mega {
    /// The first screen of a computer with no account (or a new mode of it).
    pub fn ob_show(&mut self, mode: Boot, window: &mut Window, cx: &mut Context<Self>) {
        self.screen = Screen::Welcome;
        self.ob.mode = mode; self.ob.err = None; self.ob.busy = false; self.ob.link_note.clear(); self.ob.invited = None;
        let relay = self.default_relay.clone();
        if self.ob.relay.read(cx).value().is_empty() { self.ob.relay.update(cx, |i, cx| i.set_value(relay, window, cx)); }
        let first = match mode { Boot::Invite => self.ob.ipw.clone(), Boot::Create | Boot::Pair => self.ob.name.clone(), Boot::Recover => self.ob.name.clone() };
        let first = if mode == Boot::Pair { self.ob.pp.clone() } else { first };
        first.update(cx, |i, cx| i.focus(window, cx));
        cx.notify();
    }

    fn ob_fail(&mut self, e: impl Into<String>, cx: &mut Context<Self>) { self.ob.busy = false; self.ob.err = Some(e.into().trim_start_matches("[retry] ").to_string()); cx.notify(); }

    /// What a pasted link says, as the box changes (app.js `readLink`).
    pub fn ob_read_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let v = self.ob.link.read(cx).value().trim().to_string();
        self.ob.invited = None; self.ob.link_note.clear();
        self.ob.link_seq += 1;
        if v.is_empty() { cx.notify(); return; }
        let my = self.ob.link_seq;
        self.rpc("parse_link", json!({"link": v}), window, cx, move |this, r, window, cx| {
            if my != this.ob.link_seq { return; }
            match r {
                Ok(v) => {
                    let inv = Invited { kind: v["kind"].as_str().unwrap_or("").into(), relay: v["relay"].as_str().unwrap_or("").into(), secret: v["secret"].as_str().unwrap_or("").into(), name: v["name"].as_str().unwrap_or("").into(), link: v["link"].as_str().unwrap_or("").into() };
                    let at = inv.relay.trim_start_matches("https://").trim_start_matches("http://").to_string();
                    this.ob.link_note = if this.ob.mode == Boot::Recover || this.ob.mode == Boot::Pair { format!("Server: {at}.") } else if inv.kind == "community" { format!("You are joining \u{201c}{}\u{201d} on {at}.", inv.name) } else { format!("This puts you on the relay at {at}.") };
                    let (relay, secret) = (inv.relay.clone(), inv.secret.clone());
                    this.ob.relay.update(cx, |i, cx| i.set_value(relay, window, cx));
                    this.ob.secret.update(cx, |i, cx| i.set_value(secret, window, cx));
                    this.ob.invited = Some(inv);
                }
                Err(e) => this.ob.link_note = e,
            }
            cx.notify();
        });
    }

    /// The main button of the screen in front.
    pub fn ob_go(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ob.busy || self.screen != Screen::Welcome { return; }
        let rd = |e: &Entity<InputState>, cx: &App| e.read(cx).value().to_string();
        let (ipw, name, pass, pass2, relay, secret, rk) = (rd(&self.ob.ipw, cx).trim().to_string(), rd(&self.ob.name, cx).trim().to_string(), rd(&self.ob.pp, cx), rd(&self.ob.pp2, cx), rd(&self.ob.relay, cx).trim().to_string(), rd(&self.ob.secret, cx), rd(&self.ob.rk, cx).trim().to_string());
        let mode = self.ob.mode;
        let invited = self.ob.invited.clone();
        let link_text = rd(&self.ob.link, cx).trim().to_string();
        // the checks the page makes before it asks the core
        match mode {
            Boot::Invite => {
                if ipw.is_empty() { return self.ob_fail("Type the invite password you were given.", cx); }
                if name.is_empty() { return self.ob_fail("Choose a name.", cx); }
                if pass.chars().count() < 8 { return self.ob_fail("The password for this device needs at least 8 characters.", cx); }
            }
            Boot::Create | Boot::Recover | Boot::Pair => {
                if mode == Boot::Recover && rk.is_empty() { return self.ob_fail("Paste your recovery key.", cx); }
                if mode != Boot::Pair && name.is_empty() { return self.ob_fail("Choose a name.", cx); }
                if pass.chars().count() < 8 { return self.ob_fail("The password must be at least 8 characters.", cx); }
                if pass != pass2 { return self.ob_fail("The two passphrases are different.", cx); }
            }
        }
        if !link_text.is_empty() && invited.is_none() && mode != Boot::Invite { return self.ob_fail("That is not a Megatron invite. It should start with https:// or commons://, and the whole link has to be pasted.", cx); }
        self.ob.busy = true; self.ob.err = None; cx.notify();
        let core = self.core.clone();
        let (inv2, relay2, secret2) = (invited.clone(), relay.clone(), secret.clone());
        let (pass2k, name2) = (pass.clone(), name.clone());
        // the whole sequence runs on a worker thread: each step is one call to the core, as the page makes them one after another
        let job = move || -> Result<Value, String> {
            match mode {
                Boot::Invite => {
                    if is_link(&ipw) {
                        let inv = core.call("parse_link", json!({"link": ipw}))?;
                        core.call("create_account", json!({"name": name2, "passphrase": pass2k, "relay": inv["relay"], "invite": inv["secret"].as_str().unwrap_or("")}))?;
                        let mut join_error = Value::Null;
                        if inv["kind"] == "community" { if let Err(e) = core.call("join", json!({"link": inv["link"]})) { join_error = e.into(); } }
                        core.call("recovery_saved", json!({"unsaved": true}))?;
                        Ok(json!({"join_error": join_error}))
                    } else {
                        core.call("create_account_password", json!({"password": ipw, "name": name2, "passphrase": pass2k, "relay": relay2}))
                    }
                }
                Boot::Create => {
                    core.call("create_account", json!({"name": name2, "passphrase": pass2k, "relay": relay2, "invite": secret2}))?;
                    let mut join_error = Value::Null;
                    if let Some(i) = inv2.as_ref().filter(|i| i.kind == "community") { if let Err(e) = core.call("join", json!({"link": i.link})) { join_error = e.into(); } }
                    core.call("recovery_saved", json!({"unsaved": true}))?;
                    Ok(json!({"join_error": join_error}))
                }
                Boot::Recover => core.call("recover_account", json!({"name": name2, "recovery_key": rk, "passphrase": pass2k, "relay": relay2})),
                Boot::Pair => core.call("start_pairing", json!({"passphrase": pass2k, "relay": relay2})),
            }
        };
        cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_spawn(async move { job() }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.ob.busy = false;
                match r {
                    Err(e) => this.ob_fail(e, cx),
                    Ok(v) => match mode {
                        Boot::Pair => this.ob_pairing(&v, window, cx),
                        Boot::Recover => { this.ob.kept.clear(); this.ob_clear_secrets(window, cx); this.enter_account(window, cx); }
                        Boot::Invite | Boot::Create => {
                            this.ob.kept = pass;
                            this.ob_clear_secrets(window, cx);
                            if let Some(e) = v["join_error"].as_str().filter(|e| !e.is_empty()) { this.dev.say_on_enter = Some(e.to_string()); }
                            if mode == Boot::Create { this.dev.open_key_on_enter = true; }
                            this.enter_account(window, cx);
                        }
                    },
                }
            });
        }).detach();
    }

    /// The typed passwords and keys go as soon as they have been used.
    fn ob_clear_secrets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for e in [&self.ob.ipw, &self.ob.pp, &self.ob.pp2, &self.ob.secret, &self.ob.rk, &self.ob.link] { e.update(cx, |i, cx| i.set_value("", window, cx)); }
    }

    /// `start_pairing` or `pairing_resume` answered: the code, the QR and the check digits wait for approval on another device.
    pub fn ob_pairing(&mut self, v: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let (dim, squares) = qr_squares(v["qr"].as_str().unwrap_or("")).unwrap_or((0., vec![]));
        self.ob.pair = Some(PairInfo { code: v["code"].as_str().unwrap_or("").into(), check: v["check"].as_str().unwrap_or("").into(), dim, squares });
        self.ob.pair_asked = Some(Instant::now());
        self.ob.busy = false; self.ob.err = None;
        self.ob_clear_secrets(window, cx);
        self.screen = Screen::Pairing;
        cx.notify();
    }
    /// An unlocked account with no identity is one that waits for approval (app.js `pairScreen(await rpc('pairing_resume'))`).
    pub fn ob_resume_pairing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rpc("pairing_resume", json!({}), window, cx, |this, r, window, cx| match r {
            Ok(v) => this.ob_pairing(&v, window, cx),
            Err(e) => { this.screen = Screen::Locked; this.error = Some(e.into()); cx.notify(); }
        });
    }
    /// Every quarter second: ask whether the other device has approved (the page asks every 1.5 s).
    pub fn ob_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen != Screen::Pairing { return; }
        if self.ob.pair_asked.is_some_and(|t| t.elapsed() < Duration::from_millis(1500)) { return; }
        self.ob.pair_asked = Some(Instant::now());
        self.rpc("pairing_status", json!({}), window, cx, |this, r, window, cx| {
            if this.screen == Screen::Pairing && r.is_ok_and(|v| v["done"].as_bool() == Some(true)) { this.ob.pair = None; this.enter_account(window, cx); }
        });
    }
    pub fn ob_start_over(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rpc("pairing_abandon", json!({}), window, cx, |this, _, window, cx| { this.ob.pair = None; this.look_at_state(window, cx); });
    }

    // ---- drawing ----
    fn ob_links(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let mut row = h_flex().flex_wrap().gap_x_4().gap_y_1().justify_center();
        let items: [(Boot, &'static str, &'static str); 4] = [(Boot::Invite, "l-pw", "I have an invite password"), (Boot::Create, "l-new", "I have an invite link, or I am starting a new community"), (Boot::Pair, "l-pair", "This is a new device for my account"), (Boot::Recover, "l-rec", "I only have a recovery key")];
        for (m, id, t) in items {
            if m == self.ob.mode { continue; }
            row = row.child(linkish(id, t).on_click(cx.listener(move |this, _, window, cx| this.ob_show(m, window, cx))));
        }
        row.into_any_element()
    }

    fn ob_card(&mut self, tag: &str, fields: Vec<AnyElement>, go: &str, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.ob.busy;
        let links = self.ob_links(cx);
        let relay_box = {
            let open = self.ob.relay_open || self.default_relay.is_empty() || self.ob.mode == Boot::Pair && false;
            v_flex().gap_1()
                .child(if self.default_relay.is_empty() { div().into_any_element() } else { linkish("ob-relay", if open { "Relay server (hide)" } else { "Relay server" }).on_click(cx.listener(|this, _, _, cx| { this.ob.relay_open = !this.ob.relay_open; cx.notify(); })).into_any_element() })
                .when(open, |d| d.child(Input::new(&self.ob.relay)).when(self.ob.mode != Boot::Invite, |d| d.child(lab("Invite secret (only if this relay asks for one)")).child(Input::new(&self.ob.secret))))
        };
        let err = self.ob.err.clone();
        div().id("boot-scroll").size_full().overflow_y_scroll().child(v_flex().w_full().min_h_full().items_center().justify_center().py_8().gap_3()
            .child(div().font_family(look::F_SHARP).text_size(px(96.)).line_height(px(96.)).child("mega"))
            .child(div().max_w(px(420.)).text_center().text_color(look::mute()).child(tag.to_string()))
            .child(v_flex().id("boot-card").w(px(420.)).gap_2().p_6().rounded(px(30.)).bg(rgb(look::SLIP)).border_1().border_color(look::line()).children(fields).child(relay_box)
                .when_some(err, |d, e| d.child(div().id("ob-err").text_size(px(13.)).text_color(rgb(look::DANGER)).child(e)))
                .child(div().pt_2().child(Button::new("ob-go").primary().w_full().label(if busy { "Working…".to_string() } else { go.to_string() }).disabled(busy).on_click(cx.listener(|this, _, window, cx| this.ob_go(window, cx))))))
            .child(div().max_w(px(560.)).mt_2().child(links))).into_any_element()
    }

    pub fn render_onboard(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let hint = if self.ob.hint.is_empty() { HINT0.to_string() } else { self.ob.hint.clone() };
        let note = self.ob.link_note.clone();
        let pp_pair = |me: &Self| vec![lab("A password for this device").into_any_element(), Input::new(&me.ob.pp).mask_toggle().into_any_element(), lab("Type it again").into_any_element(), Input::new(&me.ob.pp2).mask_toggle().into_any_element(), small(HINT0).into_any_element()];
        match self.ob.mode {
            Boot::Invite => {
                let f = vec![lab("Invite password").into_any_element(), Input::new(&self.ob.ipw).into_any_element(), lab("Your name").into_any_element(), Input::new(&self.ob.name).into_any_element(),
                    lab("A password for this device").into_any_element(), Input::new(&self.ob.pp).mask_toggle().into_any_element(), div().id("ob-hint").text_size(px(13.)).text_color(look::mute()).child(hint).into_any_element()];
                self.ob_card("Got an invite password? Type it, pick a name, and you are in.", f, "Join", cx)
            }
            Boot::Create => {
                let mut f = vec![lab("Invite link").into_any_element(), Input::new(&self.ob.link).into_any_element(), div().id("ob-note").text_size(px(13.)).text_color(look::mute()).child(note).into_any_element(),
                    lab("Display name").into_any_element(), Input::new(&self.ob.name).into_any_element()];
                f.extend(pp_pair(self));
                self.ob_card("Create your account on this device. Your keys are made here and never leave it.", f, "Create account", cx)
            }
            Boot::Pair => {
                let mut f = vec![];
                if self.default_relay.is_empty() { f.push(lab("Any invite link from your friends (if this device does not know the server yet)").into_any_element()); f.push(Input::new(&self.ob.link).into_any_element()); f.push(div().id("ob-note").text_size(px(13.)).text_color(look::mute()).child(note).into_any_element()); }
                f.extend(pp_pair(self));
                self.ob_card("Add this device to your account. You will get a code to approve on a device you already use.", f, "Get pairing code", cx)
            }
            Boot::Recover => {
                let mut f = vec![];
                if self.default_relay.is_empty() { f.push(lab("Any invite link from your friends (if this device does not know the server yet)").into_any_element()); f.push(Input::new(&self.ob.link).into_any_element()); f.push(div().id("ob-note").text_size(px(13.)).text_color(look::mute()).child(note).into_any_element()); }
                f.push(lab("Display name").into_any_element()); f.push(Input::new(&self.ob.name).into_any_element());
                f.push(lab("Recovery key").into_any_element()); f.push(Input::new(&self.ob.rk).into_any_element());
                f.extend(pp_pair(self));
                self.ob_card("Recover your account on this device with your recovery key. Old messages are not recoverable (they only live on devices).", f, "Recover", cx)
            }
        }
    }

    /// Waiting for a device you already use to approve this one (app.js `pairScreen`).
    pub fn render_pairing(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.ob.pair.clone().unwrap_or_default();
        let code = p.code.clone();
        let cell = if p.dim > 0. { 260. / p.dim } else { 1. };
        let side = p.dim * cell;
        let squares = p.squares.clone();
        let qr = canvas(|_, _, _| (), move |b, _, window, _| {
            window.paint_quad(fill(b, rgb(0xffffff)));
            for s in &squares { window.paint_quad(fill(Bounds::new(point(b.origin.x + px(s[0] * cell), b.origin.y + px(s[1] * cell)), size(px(s[2] * cell), px(s[3] * cell))), rgb(0x000000))); }
        }).size(px(side)).flex_none();
        div().id("boot-scroll").size_full().overflow_y_scroll().child(v_flex().w_full().min_h_full().items_center().justify_center().py_8().gap_3()
            .child(div().font_family(look::F_SHARP).text_size(px(64.)).line_height(px(64.)).child("Approve this device"))
            .child(div().max_w(px(520.)).text_center().text_color(look::mute()).child("Copy this code to a device where you already use mega (send it to yourself, for example). There, open Devices (in Settings), paste it under \u{201c}Add a device\u{201d} and press Approve."))
            .child(div().id("pair-code").max_w(px(520.)).px_4().py_3().rounded(px(18.)).border_1().border_color(look::line()).bg(rgb(look::SLIP)).font_family(look::F_MONO).text_size(px(12.)).child(p.code.clone()))
            .child(linkish("pair-copy", "Copy the code").on_click(cx.listener(move |_, _, _, cx| { cx.write_to_clipboard(ClipboardItem::new_string(code.clone())); })))
            .when(!p.squares.is_empty(), |d| d.child(div().p_2().bg(rgb(0xffffff)).child(qr)))
            .child(div().max_w(px(520.)).text_center().child(format!("Check code {}: the other device shows the same four digits when you paste the code. If they differ, do not approve.", p.check)))
            .child(div().id("pair-wait").text_color(look::mute()).child("Waiting for approval…"))
            .child(linkish("l-over", "Start over").on_click(cx.listener(|this, _, window, cx| this.ob_start_over(window, cx))))).into_any_element()
    }

    /// For the test port: what the first screens hold.
    pub fn ob_dump(&self, cx: &App) -> Value {
        json!({"screen": format!("{:?}", self.screen), "mode": format!("{:?}", self.ob.mode), "err": self.ob.err, "busy": self.ob.busy, "hint": self.ob.hint, "note": self.ob.link_note,
               "relay": self.ob.relay.read(cx).value().to_string(), "pair": self.ob.pair.as_ref().map(|p| json!({"code": p.code, "check": p.check, "squares": p.squares.len(), "dim": p.dim})), "kept": !self.ob.kept.is_empty()})
    }
}

#[cfg(test)]
mod tests {
    use super::{pp_strength, qr_squares};
    #[test]
    fn the_cores_qr_becomes_squares() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="24" height="24"><rect x="0" y="0" width="24" height="24" fill="#fff"/><path fill="#000" d="M4 4h8v8H4V4M12 16h8v8H12V16"/></svg>"##;
        let (dim, sq) = qr_squares(svg).unwrap();
        assert_eq!(dim, 24.);
        assert_eq!(sq, vec![[4., 4., 8., 8.], [12., 16., 8., 8.]]);
        assert!(qr_squares("").is_none());
    }
    #[test]
    fn password_strength_words() {
        assert_eq!(pp_strength(""), "");
        assert!(pp_strength("abc").starts_with("Too short"));
        assert!(pp_strength("password").starts_with("Weak"));
        assert_eq!(pp_strength("correct horse battery staple"), "Strong.");
    }
}
