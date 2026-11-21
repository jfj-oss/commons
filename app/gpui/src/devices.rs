//! This account's devices and what goes with them. Web reference: app.js `#btn-devices` (devices and recovery), `#btn-activity`, settings
//! (relay), mega-invite.js `showKey` (the recovery key and its reminder).
//!   * Devices and recovery: the devices (`devices`) with revoke (`revoke_device`), adding one (`pairing_preview` shows the check digits before
//!     anything is sent, `approve_pairing`), history for a new device (`history_send`, `history_decline`, `history_dismiss`), the recovery key
//!   * The recovery key (`recovery_key`, `recovery_saved`): asked for with the device password, shown, and a card in the corner that asks
//!     until the key has been saved
//!   * The activity log (`notices`)
//!   * The relay (`set_relay`, `set_relay_invite`) in settings
use crate::app::Mega;
use crate::look;
use crate::members::{danger_pill, label, note, pill, Act, Panel};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Device { pub device: String, pub created: f64, pub revoked: bool, pub this: bool }
#[derive(Clone, Debug, Default)]
pub struct Notice { pub ts: f64, pub kind: String, pub text: String }

pub struct DevUi {
    pub devices: Vec<Device>,
    at: Option<Instant>,
    loading: bool,
    pub code: Entity<InputState>,
    pub check: String,
    pub send_history: bool,
    pub approved: Option<String>,
    pub key_pw: Entity<InputState>,
    pub key: Option<String>,
    pub key_err: Option<String>,
    pub notices: Vec<Notice>,
    pub relay: Entity<InputState>,
    pub relay_secret: Entity<InputState>,
    pub relay_say: Option<(String, bool)>,
    /// A new account (made on the "create" screen) is shown its recovery key once it is in; a note from the sign-up to say once there.
    pub open_key_on_enter: bool,
    pub say_on_enter: Option<String>,
    pub busy: bool,
}

impl DevUi {
    pub fn new(window: &mut Window, cx: &mut Context<Mega>) -> (DevUi, Vec<Subscription>) {
        let code = cx.new(|cx| InputState::new(window, cx).placeholder("Paste the code shown on the new device (commons-pair:…)"));
        let key_pw = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("The password for this device"));
        let relay = cx.new(|cx| InputState::new(window, cx).placeholder("https://chat.example.org"));
        let relay_secret = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("Paste it here if you have it"));
        let subs = vec![
            cx.subscribe_in(&code, window, |this: &mut Mega, _, ev: &InputEvent, window, cx| { if let InputEvent::Change = ev { this.dev_preview(window, cx); } }),
            cx.subscribe_in(&key_pw, window, |this: &mut Mega, _, ev: &InputEvent, window, cx| { if let InputEvent::PressEnter { .. } = ev { this.dev_key_show(window, cx); } }),
        ];
        (DevUi { devices: vec![], at: None, loading: false, code, check: String::new(), send_history: true, approved: None, key_pw, key: None, key_err: None, notices: vec![], relay, relay_secret, relay_say: None, open_key_on_enter: false, say_on_enter: None, busy: false }, subs)
    }
}

fn when_text(ts: f64) -> String {
    use chrono::TimeZone as _;
    chrono::Local.timestamp_opt(ts as i64, 0).single().map(|t| t.format("%b %-d, %H:%M").to_string()).unwrap_or_default()
}
fn day_text(ts: f64) -> String {
    use chrono::TimeZone as _;
    chrono::Local.timestamp_opt(ts as i64, 0).single().map(|t| t.format("%-d %b %Y").to_string()).unwrap_or_default()
}
fn short(d: &str) -> String { d.chars().take(12).collect() }

impl Mega {
    // ---- opening ----
    pub fn dev_open(&mut self, p: Panel, window: &mut Window, cx: &mut Context<Self>) {
        match &p {
            Panel::Devices => {
                self.dev.check.clear(); self.dev.approved = None; self.dev.send_history = true;
                self.dev.code.update(cx, |i, cx| i.set_value("", window, cx));
                self.dev.at = None;
                self.dev_load(window, cx);
            }
            Panel::Key => {
                self.dev.key = None; self.dev.key_err = None;
                self.dev.key_pw.update(cx, |i, cx| { i.set_value("", window, cx); i.focus(window, cx); });
                // the password just typed on the first screen shows it at once (the page's `kept`)
                let kept = std::mem::take(&mut self.ob.kept);
                if !kept.is_empty() { self.dev_key_reveal(kept, window, cx); }
            }
            Panel::Activity => { self.dev.notices.clear(); self.dev_notices(window, cx); }
            _ => {}
        }
        self.open_panel(p, window, cx);
    }

    // ---- the devices ----
    pub fn dev_load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dev.loading { return; }
        self.dev.loading = true;
        self.rpc("devices", json!({}), window, cx, |this, r, _, cx| {
            this.dev.loading = false; this.dev.at = Some(Instant::now());
            let Ok(v) = r else { return };
            let ds: Vec<Device> = v.as_array().map(|l| l.iter().map(|d| Device { device: d["device"].as_str().unwrap_or("").into(), created: d["created"].as_f64().unwrap_or(0.), revoked: d["revoked"].as_bool().unwrap_or(false), this: d["this"].as_bool().unwrap_or(false) }).collect()).unwrap_or_default();
            if ds != this.dev.devices { this.dev.devices = ds; cx.notify(); }
        });
    }
    /// Every quarter second: requests and progress change while the panel is open (the page redraws every 2.5 s).
    pub fn dev_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mem.panel == Some(Panel::Devices) && self.dev.at.map_or(true, |t| t.elapsed() > Duration::from_millis(2500)) { self.snap_hash.clear(); self.tick(window, cx); self.dev_load(window, cx); }
    }
    pub fn dev_revoke(&mut self, device: String, window: &mut Window, cx: &mut Context<Self>) {
        self.rpc("revoke_device", json!({"device": device}), window, cx, |this, r, window, cx| {
            match r { Ok(_) => this.say_members("Device revoked. It is removed from every room and cannot read anything new.", false, cx), Err(e) => this.say_members(format!("Could not revoke it: {}", e.trim_start_matches("[retry] ")), true, cx) }
            this.dev.at = None; this.dev_load(window, cx);
        });
    }
    pub fn dev_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let code = self.dev.code.read(cx).value().trim().to_string();
        self.dev.approved = None;
        if code.is_empty() { self.dev.check.clear(); cx.notify(); return; }
        self.rpc("pairing_preview", json!({"code": code.clone()}), window, cx, move |this, r, _, cx| {
            if this.dev.code.read(cx).value().trim() != code { return; }
            this.dev.check = r.ok().and_then(|v| v["check"].as_str().map(String::from)).unwrap_or_default(); cx.notify();
        });
    }
    pub fn dev_approve(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dev.busy { return; }
        let code = self.dev.code.read(cx).value().trim().to_string();
        if code.is_empty() { self.say_members("Paste the code shown on the new device.", true, cx); return; }
        self.dev.busy = true;
        let h = self.dev.send_history;
        self.rpc("approve_pairing", json!({"code": code, "send_history": h}), window, cx, |this, r, window, cx| {
            this.dev.busy = false;
            match r {
                Ok(v) => { let chk = v["check"].as_str().unwrap_or("").to_string(); this.dev.approved = Some(chk); this.mem.say = None; this.dev.code.update(cx, |i, cx| i.set_value("", window, cx)); this.dev.check.clear(); }
                Err(e) => this.say_members(e.trim_start_matches("[retry] ").to_string(), true, cx),
            }
            cx.notify();
        });
    }
    /// History for a new device: send what this device holds, leave the request, or stop showing the note about a transfer.
    pub fn dev_history(&mut self, method: &'static str, device: String, window: &mut Window, cx: &mut Context<Self>) {
        self.rpc(method, json!({"device": device}), window, cx, |this, r, window, cx| {
            if let Err(e) = r { this.say_members(e.trim_start_matches("[retry] ").to_string(), true, cx); }
            this.snap_hash.clear(); this.tick(window, cx); cx.notify();
        });
    }

    // ---- the recovery key ----
    pub fn dev_key_show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pw = self.dev.key_pw.read(cx).value().to_string();
        self.dev_key_reveal(pw, window, cx);
    }
    fn dev_key_reveal(&mut self, pass: String, window: &mut Window, cx: &mut Context<Self>) {
        self.rpc("recovery_key", json!({"passphrase": pass}), window, cx, |this, r, window, cx| {
            match r {
                Ok(v) => { this.dev.key = v["key"].as_str().map(String::from); this.dev.key_err = None; this.dev.key_pw.update(cx, |i, cx| i.set_value("", window, cx)); }
                Err(e) => this.dev.key_err = Some(if e.contains("wrong") || e.contains("Wrong") { "Wrong password.".into() } else { e }),
            }
            cx.notify();
        });
    }
    pub fn dev_key_saved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rpc("recovery_saved", json!({"unsaved": false}), window, cx, |this, _, window, cx| {
            this.ob.kept.clear(); this.dev.key = None; this.mem.panel = None; this.snap_hash.clear(); this.tick(window, cx); cx.notify();
        });
    }

    // ---- the activity log ----
    fn dev_notices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rpc("notices", json!({}), window, cx, |this, r, _, cx| {
            if let Ok(v) = r { this.dev.notices = v.as_array().map(|l| l.iter().map(|n| Notice { ts: n["ts"].as_f64().unwrap_or(0.), kind: n["kind"].as_str().unwrap_or("").into(), text: n["text"].as_str().unwrap_or("").into() }).collect()).unwrap_or_default(); }
            cx.notify();
        });
    }

    // ---- the relay (settings) ----
    pub fn dev_relay_fill(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let r = self.snap.as_ref().map(|s| s.relay.clone()).unwrap_or_default();
        self.dev.relay.update(cx, |i, cx| i.set_value(r, window, cx));
        self.dev.relay_secret.update(cx, |i, cx| i.set_value("", window, cx));
        self.dev.relay_say = None;
    }
    pub fn dev_relay_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let was = self.snap.as_ref().map(|s| s.relay.clone()).unwrap_or_default();
        let (relay, secret) = (self.dev.relay.read(cx).value().trim().to_string(), self.dev.relay_secret.read(cx).value().trim().to_string());
        let core = self.core.clone();
        self.dev.busy = true;
        cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_spawn(async move {
                if relay != was && !relay.is_empty() { core.call("set_relay", json!({"relay": relay}))?; }
                if !secret.is_empty() { core.call("set_relay_invite", json!({"secret": secret}))?; }
                Ok::<(), String>(())
            }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.dev.busy = false;
                this.dev.relay_say = Some(match r { Ok(()) => ("Saved.".into(), false), Err(e) => (e.trim_start_matches("[retry] ").to_string(), true) });
                this.dev.relay_secret.update(cx, |i, cx| i.set_value("", window, cx));
                this.snap_hash.clear(); this.tick(window, cx); cx.notify();
            });
        }).detach();
    }
    /// The relay part of settings: where the relay is (changed only when it moved), and the server's sign-up secret.
    pub fn relay_settings(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let has = self.snap.as_ref().is_some_and(|s| s.relay_secret);
        let acct = self.snap.as_ref().map(|s| s.me_account.chars().take(16).collect::<String>()).unwrap_or_default();
        let say = self.dev.relay_say.clone();
        v_flex().gap_2()
            .child(label("relay address")).child(Input::new(&self.dev.relay))
            .child(note(format!("Change this only if your relay moved. A different relay does not have your communities. Your account id: {acct}…")))
            .child(label("this server's sign-up secret")).child(Input::new(&self.dev.relay_secret))
            .child(note(if has { "Saved on this device. Leave empty to keep it." } else { "Not saved on this device. Paste it here if you have it. A device added by pairing may not have it; without it, invite links from this device only work for people who already have an account here." }))
            .child(h_flex().gap_2().items_center().child(pill("relay-save", false, "Save relay").on_click(cx.listener(|this, _, window, cx| this.dev_relay_save(window, cx))))
                .when_some(say, |d, (t, bad)| d.child(div().id("relay-say").text_size(px(13.)).text_color(if bad { rgb(look::DANGER).into() } else { look::mute() }).child(t))))
            .into_any_element()
    }

    // ---- drawing ----
    pub fn dev_panel(&mut self, p: Panel, cx: &mut Context<Self>) -> AnyElement {
        match p { Panel::Devices => self.devices_view(cx), Panel::Key => self.key_view(cx), _ => self.activity_view(cx) }
    }

    fn devices_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let h = self.snap.as_ref().map(|s| s.history.clone()).unwrap_or(Value::Null);
        let rooms = h["here"]["rooms"].as_u64().unwrap_or(0); let msgs = h["here"]["msgs"].as_u64().unwrap_or(0);
        let mut reqs = v_flex().gap_2();
        for r in h["requests"].as_array().cloned().unwrap_or_default() {
            let d = r["device"].as_str().unwrap_or("").to_string();
            let (a, b) = (d.clone(), d.clone());
            reqs = reqs.child(v_flex().id(SharedString::from(format!("hreq-{d}"))).gap_2().px_3().py_2().rounded(px(18.)).border_1().border_color(look::line())
                .child(div().text_size(px(14.)).child(format!("Your new device {} asks for this device's message history ({msgs} messages in {rooms} rooms).", short(&d))))
                .child(h_flex().gap_2()
                    .child(pill(SharedString::from(format!("hsend-{d}")), true, "Send").on_click(cx.listener(move |this, _, window, cx| this.dev_history("history_send", a.clone(), window, cx))))
                    .child(pill(SharedString::from(format!("hnot-{d}")), false, "Not now").on_click(cx.listener(move |this, _, window, cx| this.dev_history("history_decline", b.clone(), window, cx))))));
        }
        for x in h["sending"].as_array().cloned().unwrap_or_default() {
            let d = x["device"].as_str().unwrap_or("");
            reqs = reqs.child(note(if x["done"].as_bool() == Some(true) { format!("History sent to {}.", short(d)) } else { format!("Sending history to {}: {} of {} parts.", short(d), x["sent"].as_u64().unwrap_or(0), x["total"].as_u64().unwrap_or(0)) }));
        }
        let mut list = v_flex().gap_1();
        for d in self.dev.devices.clone() {
            let id = d.device.clone();
            list = list.child(h_flex().id(SharedString::from(format!("dev-{}", d.device))).gap_3().items_center().px_3().py_2().rounded(px(18.)).border_1().border_color(look::line())
                .child(v_flex().flex_1().min_w_0().child(div().font_family(look::F_MONO).text_size(px(15.)).child(short(&d.device)))
                    .child(div().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child(format!("added {}{}", day_text(d.created), if d.this { " · this device" } else { "" }))))
                .child(if d.revoked { div().px_2().rounded_full().border_1().border_color(look::line()).font_family(look::F_NAME).text_size(px(11.)).text_color(look::mute()).child("revoked").into_any_element() }
                    else if d.this { div().into_any_element() }
                    else { danger_pill(SharedString::from(format!("rev-{}", d.device)), "Revoke").on_click(cx.listener(move |this, _, _, cx| {
                        this.ask("Revoke this device?", format!("{} is removed from every room and cannot read anything new. Do this for a lost or stolen device.", short(&id)), "Revoke", Act::RevokeDevice { device: id.clone() }, cx);
                    })).into_any_element() }));
        }
        if self.dev.devices.is_empty() { list = list.child(note("loading…")); }
        let check = self.dev.check.clone();
        let approved = self.dev.approved.clone();
        let hist = self.dev.send_history;
        let busy = self.dev.busy;
        v_flex().gap_3().child(self.title("Devices and recovery", "every device holds its own keys", cx))
            .child(reqs)
            .child(note("Every device holds its own keys. A lost device must be revoked: it is then removed from every room and cannot read anything new."))
            .child(list)
            .child(label("add a device"))
            .child(Input::new(&self.dev.code))
            .when(!check.is_empty(), |d| d.child(div().id("dev-check").text_size(px(13.)).text_color(look::mute()).child(format!("Check code {check}. It must match the new device before you approve."))))
            .when_some(approved, |d, c| d.child(div().id("dev-approved").text_size(px(13.)).child(format!("Approved. The new device will join your rooms within a few seconds. Check code: {c}"))))
            .child(h_flex().id("dev-hist").gap_2().items_center().cursor_pointer().on_click(cx.listener(|this, _, _, cx| { this.dev.send_history = !this.dev.send_history; cx.notify(); }))
                .child(div().size(px(16.)).rounded(px(4.)).border_1().border_color(look::paper()).when(hist, |d| d.bg(look::paper())))
                .child(div().text_size(px(13.)).child("Also send this device's message history to it")))
            .child(h_flex().gap_2().flex_wrap()
                .child(pill("dev-approve", true, if busy { "Approving…" } else { "Approve new device" }).on_click(cx.listener(|this, _, window, cx| this.dev_approve(window, cx))))
                .child(pill("dev-key", false, "Show my recovery key").on_click(cx.listener(|this, _, window, cx| this.dev_open(Panel::Key, window, cx))))
                .child(pill("dev-log", false, "Activity log").on_click(cx.listener(|this, _, window, cx| this.dev_open(Panel::Activity, window, cx)))))
            .into_any_element()
    }

    fn key_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let unsaved = self.snap.as_ref().is_some_and(|s| s.recovery_unsaved);
        let key = self.dev.key.clone();
        let err = self.dev.key_err.clone();
        let body = match key.clone() {
            None => v_flex().gap_2().child(note("Type the password for this device to show the key.")).child(Input::new(&self.dev.key_pw).mask_toggle())
                .when_some(err, |d, e| d.child(div().id("key-err").text_size(px(13.)).text_color(rgb(look::DANGER)).child(e)))
                .child(h_flex().child(pill("key-show", true, "Show key").on_click(cx.listener(|this, _, window, cx| this.dev_key_show(window, cx))))).into_any_element(),
            Some(k) => { let c = k.clone(); v_flex().gap_2()
                .child(div().text_size(px(15.)).child("If this computer is lost or broken, this key gets you back into your account on a new one, and your communities let you back in. Messages from before do not come back."))
                .child(div().id("key-code").px_4().py_3().rounded(px(18.)).border_1().border_color(look::line()).bg(rgb(look::VOID)).font_family(look::F_MONO).text_size(px(15.)).child(k))
                .child(h_flex().child(pill("key-copy", false, "Copy").on_click(cx.listener(move |this, _, _, cx| { cx.write_to_clipboard(ClipboardItem::new_string(c.clone())); this.say_members("Key copied.", false, cx); }))))
                .child(note("Write it down, or keep it in a password manager, somewhere that is not this computer. Anyone who has it can act as you. You can see it again under Devices.")).into_any_element() }
        };
        v_flex().gap_3().child(self.title("Save your recovery key", if unsaved { "not saved yet" } else { "your way back in" }, cx))
            .child(body)
            .child(h_flex().gap_2().justify_end()
                .child(pill("key-later", false, if unsaved { "Later" } else { "Close" }).on_click(cx.listener(|this, _, _, cx| { this.ob.kept.clear(); this.dev.key = None; this.close_panel(cx); })))
                .when(unsaved && key.is_some(), |d| d.child(pill("key-saved", true, "I saved it").on_click(cx.listener(|this, _, window, cx| this.dev_key_saved(window, cx))))))
            .into_any_element()
    }

    fn activity_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let mut rows = v_flex().gap_1();
        for n in &self.dev.notices {
            let bad = ["decrypt", "broken", "rollback", "reject", "gap"].contains(&n.kind.as_str());
            rows = rows.child(h_flex().gap_3().items_start().px_3().py_1().rounded(px(12.)).border_1().border_color(if bad { rgb(look::DANGER).into() } else { look::line() })
                .child(div().w(px(80.)).flex_none().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child(n.kind.clone()))
                .child(div().flex_1().min_w_0().text_size(px(14.)).child(n.text.clone()))
                .child(div().flex_none().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child(when_text(n.ts))));
        }
        v_flex().gap_3().child(self.title("Activity log", "everything unusual the app noticed", cx))
            .child(note("In plain words. Nothing here is hidden from you: if a message could not be read, it says why."))
            .child(if self.dev.notices.is_empty() { div().id("log-none").text_color(look::mute()).child("Nothing yet.").into_any_element() } else { div().id("log-rows").child(rows).into_any_element() })
            .into_any_element()
    }

    /// The card in the corner that asks, with no close button, until the recovery key has been saved (mega-invite.js `reminder`).
    pub fn recovery_card(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.snap.as_ref().is_some_and(|s| s.recovery_unsaved) || self.mem.panel == Some(Panel::Key) { return None; }
        Some(div().id("rec-card").absolute().left(px(16.)).bottom(px(16.)).occlude().child(h_flex().gap_3().items_center().max_w(px(440.)).px_4().py_3().rounded(px(22.)).bg(rgb(look::SLIP)).border_1().border_color(look::line()).shadow_lg()
            .child(v_flex().flex_1().min_w_0().child(div().font_family(look::F_NAME).text_size(px(14.)).child("Save your recovery key"))
                .child(div().text_size(px(13.)).text_color(look::mute()).child("Without it, a lost computer means a lost account.")))
            .child(pill("rec-go", true, "Show my key").on_click(cx.listener(|this, _, window, cx| this.dev_open(Panel::Key, window, cx))))).into_any_element())
    }

    /// The home screen's notes about this account's message history on a new device (app.js `historyNote`).
    pub fn history_notes(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(h) = self.snap.as_ref().map(|s| s.history["incoming"].clone()) else { return vec![] };
        let state = h["state"].as_str().unwrap_or("none");
        if state == "none" { return vec![]; }
        let (text, dismiss) = match state {
            "receiving" => (format!("Your other device is sending this device's message history: {} of {} parts.", h["got"].as_u64().unwrap_or(0), h["total"].as_u64().unwrap_or(0)), false),
            "done" => (format!("History from your other device arrived: {} messages.{}", h["msgs"].as_u64().unwrap_or(0), if h["omitted"].as_u64().unwrap_or(0) > 0 { format!(" The oldest {} stayed on the other device.", h["omitted"].as_u64().unwrap_or(0)) } else { String::new() }), true),
            "waiting" => ("No other device has answered yet about your message history. Open mega on a device you already use and press Send under Devices.".to_string(), true),
            _ => ("Asking your other devices for your message history…".to_string(), false),
        };
        vec![h_flex().id("hist-note").gap_3().items_center().px_4().py_2().rounded(px(18.)).bg(rgb(look::SLIP)).border_1().border_color(look::line())
            .child(div().flex_1().font_family(look::F_NAME).text_size(px(13.)).child(text))
            .when(dismiss, |d| d.child(pill("hist-x", false, "dismiss").on_click(cx.listener(|this, _, window, cx| this.dev_history("history_dismiss", String::new(), window, cx))))).into_any_element()]
    }

    /// For the test port: what the devices, key and log panels hold.
    pub fn dev_dump(&self, cx: &App) -> Value {
        json!({"panel": format!("{:?}", self.mem.panel), "devices": self.dev.devices.iter().map(|d| json!({"device": d.device, "revoked": d.revoked, "this": d.this})).collect::<Vec<_>>(),
               "check": self.dev.check, "approved": self.dev.approved, "key": self.dev.key, "key_err": self.dev.key_err, "notices": self.dev.notices.iter().map(|n| json!({"kind": n.kind, "text": n.text})).collect::<Vec<_>>(),
               "recovery_unsaved": self.snap.as_ref().map(|s| s.recovery_unsaved), "history": self.snap.as_ref().map(|s| s.history.clone()), "relay": self.dev.relay.read(cx).value().to_string(), "relay_say": self.dev.relay_say,
               "direct_refused": self.snap.as_ref().map(|s| s.direct_refused.iter().map(|(a, n)| json!({"account": a, "name": n})).collect::<Vec<_>>()),
               "directs": self.snap.as_ref().map(|s| s.directs.iter().map(|d| json!({"peer": d.peer, "name": d.name, "supported": d.supported})).collect::<Vec<_>>())})
    }
}
