//! Game keys, as the web's dialog (app.js `gameKeys`): per-game push-to-mute. While a listed program runs and its key is held,
//! the microphone is off in the call. The rules live in the core (`game_keys_get` / `game_keys_set`, `running_programs`,
//! `game_keys_allow`; the watcher is commons-gamekeys); this is only the dialog. Opened from Settings ("Calls") and from the
//! call settings. The key to set is taken from this window's own key events (and the back / forward mouse buttons), named as
//! the browser names a physical key (`KeyF`, `Digit1`, `F5`, `Mouse4`) because that is what the core stores.
use crate::app::Mega;
use crate::look;
use crate::squircle as sq;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct Row { pub process: Option<String>, pub key: String, pub mode: String }

/// The dialog's data, with no window in it (so it can be tested).
#[derive(Clone, Debug)]
pub struct Data {
    /// The "any app" rule first, then one per named game.
    pub any: Row,
    pub rows: Vec<Row>,
    pub portal: bool,
    pub portal_possible: bool,
    pub note: String,
    pub permission: bool,
}
impl Default for Data { fn default() -> Data { Data { any: Row { process: None, key: String::new(), mode: "hold".into() }, rows: vec![], portal: false, portal_possible: false, note: String::new(), permission: false } } }

pub struct GameKeys {
    pub open: bool,
    pub d: Data,
    pub programs: Vec<String>,
    /// Which row waits for a key: -1 any app, else an index of `rows`.
    pub capture: Option<i32>,
    pub error: Option<String>,
    pub add: Entity<InputState>,
    pub focus: FocusHandle,
    pub hits: crate::calldock::Hits,
}

impl GameKeys {
    pub fn new(window: &mut Window, cx: &mut Context<Mega>) -> GameKeys {
        GameKeys { open: false, d: Data::default(), programs: vec![], capture: None, error: None, add: cx.new(|cx| InputState::new(window, cx).placeholder("Start typing, or pick one")), focus: cx.focus_handle(), hits: Default::default() }
    }
}

impl Data {
    /// The settings the core takes: the any-app rule when it has a key, then the named games.
    pub fn to_settings(&self) -> Result<Value, String> {
        if let Some(r) = self.rows.iter().find(|r| r.key.is_empty()) {
            return Err(format!("Press \u{201c}Press to set\u{201d} next to {} and then its voice key, or remove it with \u{d7}.", r.process.clone().unwrap_or_default()));
        }
        let rule = |r: &Row| json!({"process": r.process, "key": r.key, "mode": r.mode});
        let mut rules: Vec<Value> = vec![];
        if !self.any.key.is_empty() { rules.push(rule(&self.any)); }
        rules.extend(self.rows.iter().map(rule));
        Ok(json!({"rules": rules, "use_portal": self.portal && self.portal_possible}))
    }
    pub fn load(&mut self, v: &Value) {
        let rules = v["settings"]["rules"].as_array().cloned().unwrap_or_default();
        let row = |r: &Value| Row { process: r["process"].as_str().map(String::from), key: r["key"].as_str().unwrap_or("").to_string(), mode: if r["mode"].as_str() == Some("toggle") { "toggle".into() } else { "hold".into() } };
        self.any = rules.iter().find(|r| r["process"].is_null()).map(row).unwrap_or(Row { process: None, key: String::new(), mode: "hold".into() });
        self.rows = rules.iter().filter(|r| r["process"].as_str().is_some_and(|p| !p.is_empty())).map(row).collect();
        self.portal = v["settings"]["use_portal"].as_bool() == Some(true);
        self.portal_possible = v["portal_possible"].as_bool() == Some(true);
        self.permission = v["permission"].as_bool() == Some(true);
        let hold = v["hold"].as_bool() != Some(false);
        self.note = format!("{}{}{}", v["note"].as_str().unwrap_or(""), if hold { "" } else { " Holding a key cannot be seen here." },
            v["error"].as_str().filter(|e| !e.is_empty()).map(|e| format!(" Last problem: {e}")).unwrap_or_default());
    }
    /// Add a program to the list (once, whatever its case), with no key yet.
    pub fn add_program(&mut self, name: &str) {
        let n = name.trim();
        if n.is_empty() || self.rows.iter().any(|r| r.process.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(n))) { return; }
        self.rows.push(Row { process: Some(n.to_string()), key: String::new(), mode: "hold".into() });
    }
    pub fn row_mut(&mut self, i: i32) -> Option<&mut Row> { if i < 0 { Some(&mut self.any) } else { self.rows.get_mut(i as usize) } }
}

/// What the page shows on a key's button (`keyName`).
pub fn key_name(code: &str) -> String {
    if code.is_empty() { return "Press to set".into(); }
    let c = code.strip_prefix("Key").or_else(|| code.strip_prefix("Digit")).unwrap_or(code);
    if let Some(n) = c.strip_prefix("Mouse") { return format!("Mouse button {n}"); }
    for side in ["Left", "Right"] { if let Some(b) = c.strip_suffix(side) { if !b.is_empty() && b != "Arrow" { return format!("{b} {side}"); } } }
    c.to_string()
}

/// A key event's key, as the browser's physical-key name the core stores (None: not a key the core knows).
pub fn code_of(key: &str) -> Option<String> {
    let k = key.to_lowercase();
    let c = match k.as_str() {
        "space" => "Space", "tab" => "Tab", "enter" => "Enter", "backspace" => "Backspace", "escape" => "Escape",
        "left" => "ArrowLeft", "right" => "ArrowRight", "up" => "ArrowUp", "down" => "ArrowDown",
        "insert" => "Insert", "delete" => "Delete", "home" => "Home", "end" => "End", "pageup" => "PageUp", "pagedown" => "PageDown",
        "`" => "Backquote", "-" => "Minus", "=" => "Equal", "[" => "BracketLeft", "]" => "BracketRight", "\\" => "Backslash",
        ";" => "Semicolon", "'" => "Quote", "," => "Comma", "." => "Period", "/" => "Slash",
        _ => "",
    };
    if !c.is_empty() { return commons_gamekeys::keys::known(c).then(|| c.to_string()); }
    let ch: Vec<char> = k.chars().collect();
    if ch.len() == 1 && ch[0].is_ascii_lowercase() { return Some(format!("Key{}", ch[0].to_ascii_uppercase())); }
    if ch.len() == 1 && ch[0].is_ascii_digit() { return Some(format!("Digit{}", ch[0])); }
    if let Some(n) = k.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) { let c = format!("F{n}"); return commons_gamekeys::keys::known(&c).then_some(c); }
    None
}

impl Mega {
    pub fn gk_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.gk.open = true; self.gk.capture = None; self.gk.error = None;
        self.gk.add.update(cx, |i, cx| i.set_value("", window, cx));
        self.rpc("game_keys_get", json!({}), window, cx, |this, r, _, cx| {
            match r { Ok(v) => this.gk.d.load(&v), Err(e) => this.gk.error = Some(e) }
            cx.notify();
        });
        self.rpc("running_programs", json!({}), window, cx, |this, r, _, cx| {
            if let Ok(v) = r { this.gk.programs = v.as_array().map(|l| l.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(); cx.notify(); }
        });
        cx.notify();
    }
    pub fn gk_close(&mut self, cx: &mut Context<Self>) { self.gk.open = false; self.gk.capture = None; cx.notify(); }
    fn gk_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.gk.d.to_settings() {
            Err(e) => { self.gk.error = Some(e); cx.notify(); }
            Ok(args) => self.rpc("game_keys_set", args, window, cx, |this, r, _, cx| {
                match r { Ok(_) => { this.gk.open = false; this.gk.error = None; this.say("Game keys saved", cx); } Err(e) => this.gk.error = Some(e) }
                cx.notify();
            }),
        }
    }
    /// A key (or mouse button) arrived while a row waits for one. Escape leaves things as they were.
    fn gk_got(&mut self, code: Option<String>, cx: &mut Context<Self>) {
        let Some(i) = self.gk.capture.take() else { return };
        if let Some(c) = code { if let Some(r) = self.gk.d.row_mut(i) { r.key = c; } }
        cx.notify();
    }

    pub fn gk_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.gk.open { return None; }
        let hits = self.gk.hits.clone();
        let g = &self.gk;
        let d = g.d.clone();
        let capturing = g.capture;
        let row = |i: i32, r: &Row, cx: &mut Context<Self>| {
            let id = if i < 0 { "gk-any".to_string() } else { format!("gk-{i}") };
            let name = r.process.clone().unwrap_or_else(|| "Any app".into());
            let (mode, hits2, hits3) = (r.mode.clone(), hits.clone(), hits.clone());
            let wait = capturing == Some(i);
            h_flex().relative().w_full().gap(px(10.)).items_center()
                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_family(look::F_UI).text_size(px(13.)).child(name))
                .child(div().id(SharedString::from(format!("{id}-key"))).relative().min_w(px(120.)).h(px(32.)).px(px(10.)).flex().items_center().justify_center().cursor_pointer().font_family(look::F_UI).text_size(px(13.))
                    .child(sq::back(10., if wait { look::paper() } else { rgb(0x0b0a10).into() })).child(sq::front(10., None, Some((1., look::line())), None)).child(hits2.track(format!("{id}-key")))
                    .child(div().relative().when(wait, |d| d.text_color(rgb(0x0c0d12))).child(if wait { "Press a key\u{2026}".to_string() } else { key_name(&r.key) }))
                    .on_click(cx.listener(move |this, _, window, cx| { this.gk.capture = Some(i); this.gk.error = None; window.focus(&this.gk.focus, cx); cx.notify(); })))
                .child(div().id(SharedString::from(format!("{id}-mode"))).relative().h(px(32.)).px(px(10.)).flex().items_center().cursor_pointer().font_family(look::F_UI).text_size(px(13.))
                    .child(sq::back(10., rgb(0x0b0a10))).child(sq::front(10., None, Some((1., look::line())), None)).child(hits3.track(format!("{id}-mode")))
                    .child(div().relative().child(if mode == "toggle" { "Press to toggle" } else { "Hold to mute" }))
                    .on_click(cx.listener(move |this, _, _, cx| { if let Some(r) = this.gk.d.row_mut(i) { r.mode = if r.mode == "toggle" { "hold".into() } else { "toggle".into() }; } cx.notify(); })))
                .child(if i >= 0 {
                    div().id(SharedString::from(format!("{id}-del"))).relative().size(px(28.)).flex().items_center().justify_center().cursor_pointer().text_size(px(18.)).text_color(look::mute()).child("\u{d7}").child(hits.track(format!("{id}-del")))
                        .on_click(cx.listener(move |this, _, _, cx| { if (i as usize) < this.gk.d.rows.len() { this.gk.d.rows.remove(i as usize); } cx.notify(); })).into_any_element()
                } else { div().size(px(28.)).into_any_element() })
        };
        let mut list = v_flex().w_full().gap(px(8.)).child(row(-1, &d.any, cx));
        for (i, r) in d.rows.iter().enumerate() { list = list.child(row(i as i32, r, cx)); }
        let typed = g.add.read(cx).value().to_string().to_lowercase();
        let picks: Vec<String> = g.programs.iter().filter(|p| !d.rows.iter().any(|r| r.process.as_deref() == Some(p.as_str())) && (typed.is_empty() || p.to_lowercase().contains(&typed))).take(10).cloned().collect();
        let mut chips = h_flex().flex_wrap().gap(px(6.));
        for (i, p) in picks.into_iter().enumerate() {
            let p2 = p.clone();
            chips = chips.child(div().id(SharedString::from(format!("gk-pick-{i}"))).relative().px(px(10.)).py(px(3.)).rounded_full().border_1().border_color(look::line()).cursor_pointer().font_family(look::F_UI).text_size(px(12.))
                .child(div().max_w(px(280.)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(p)).child(hits.track(format!("gk-pick-{i}")))
                .on_click(cx.listener(move |this, _, _, cx| { this.gk.d.add_program(&p2); cx.notify(); })));
        }
        let portal = d.portal;
        let body = v_flex().w_full().gap(px(12.)).px(px(22.)).py(px(14.))
            .child(div().font_family(look::F_UI).text_size(px(13.)).text_color(look::mute()).child("While the game runs and you hold its key, your mic is off in the mega call, so friends do not hear what you say to your team. Set the game's own voice key here too. These settings stay on this device."))
            .child(list)
            .child(div().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child("add a game (a running program)"))
            .child(h_flex().gap(px(8.)).items_center().child(div().flex_1().child(Input::new(&g.add)))
                .child(div().id("gk-addb").relative().h(px(32.)).px(px(14.)).flex().items_center().cursor_pointer().font_family(look::F_UI).text_size(px(13.)).child(sq::back(10., rgb(0x0b0a10))).child(sq::front(10., None, Some((1., look::line())), None)).child(div().relative().child("Add")).child(hits.track("gk-addb"))
                    .on_click(cx.listener(|this, _, window, cx| { let v = this.gk.add.read(cx).value().to_string(); this.gk.d.add_program(&v); this.gk.add.update(cx, |i, cx| i.set_value("", window, cx)); cx.notify(); }))))
            .child(chips)
            .child(div().id("gk-note").font_family(look::F_UI).text_size(px(12.)).text_color(look::mute()).child(d.note.clone()))
            .when(d.permission, |d| d.child(div().id("gk-allow").relative().self_start().h(px(32.)).px(px(14.)).flex().items_center().cursor_pointer().font_family(look::F_UI).text_size(px(13.)).child(sq::back(10., rgb(0x0b0a10))).child(sq::front(10., None, Some((1., look::line())), None)).child(div().relative().child("Allow Input Monitoring"))
                .on_click(cx.listener(|this, _, window, cx| this.rpc("game_keys_allow", json!({}), window, cx, |this, r, _, cx| { this.say(&match r { Ok(_) => "If macOS asked, switch mega on, then restart mega.".to_string(), Err(e) => e }, cx); })))))
            .when(d.portal_possible, |d| d.child(div().id("gk-portal").relative().flex().gap(px(10.)).items_start().cursor_pointer().font_family(look::F_UI).text_size(px(13.)).child(hits.track("gk-portal"))
                .child(div().relative().mt(px(2.)).size(px(16.)).flex_none().rounded(px(4.)).border_1().border_color(look::line()).when(portal, |d| d.bg(look::paper())))
                .child(div().flex_1().min_w_0().child("Use system shortcuts (the desktop asks you once). Your game may stop receiving a key bound this way."))
                .on_click(cx.listener(|this, _, _, cx| { this.gk.d.portal = !this.gk.d.portal; cx.notify(); }))))
            .children(g.error.clone().map(|e| div().id("gk-error").text_size(px(13.)).text_color(rgb(look::DANGER)).child(e)));
        let btn = |id: &'static str, label: &'static str, primary: bool| div().id(id).relative().h(px(34.)).px(px(18.)).flex().items_center().cursor_pointer().font_family(look::F_UI).text_size(px(14.)).font_weight(FontWeight::SEMIBOLD)
            .child(sq::back(12., if primary { look::paper() } else { rgb(0x0b0a10).into() })).child(sq::front(12., None, Some((1., look::line())), None)).child(div().relative().when(primary, |d| d.text_color(rgb(0x0c0d12))).child(label)).child(hits.track(id));
        let panel = v_flex().id("gk-panel").relative().w(px(640.)).max_w_full().max_h(relative(0.9)).overflow_hidden().occlude().track_focus(&self.gk.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                if this.gk.capture.is_some() { cx.stop_propagation(); let k = ev.keystroke.key.as_str(); this.gk_got(if k == "escape" { None } else { code_of(k) }, cx); }
                else if ev.keystroke.key == "escape" { cx.stop_propagation(); this.gk_close(cx); }
            }))
            .on_mouse_down(MouseButton::Navigate(NavigationDirection::Back), cx.listener(|this, _, _, cx| if this.gk.capture.is_some() { cx.stop_propagation(); this.gk_got(Some("Mouse4".into()), cx); }))
            .on_mouse_down(MouseButton::Navigate(NavigationDirection::Forward), cx.listener(|this, _, _, cx| if this.gk.capture.is_some() { cx.stop_propagation(); this.gk_got(Some("Mouse5".into()), cx); }))
            .child(sq::back(28., rgb(0x14131a))).child(sq::front(28., None, Some((1., look::line())), None)).on_click(|_, _, cx| cx.stop_propagation())
            .child(h_flex().relative().w_full().items_center().gap(px(10.)).px(px(22.)).py(px(12.)).border_b_1().border_color(look::line())
                .child(div().flex_1().font_family(look::F_SHARP).text_size(px(24.)).line_height(px(26.)).child("game keys")))
            .child(div().id("gk-scroll").relative().flex_1().min_h_0().overflow_y_scroll().child(body))
            .child(h_flex().relative().w_full().justify_end().gap(px(10.)).px(px(22.)).py(px(12.)).border_t_1().border_color(look::line())
                .child(btn("gk-cancel", "Cancel", false).on_click(cx.listener(|this, _, _, cx| this.gk_close(cx))))
                .child(btn("gk-save", "Save", true).on_click(cx.listener(|this, _, window, cx| this.gk_save(window, cx)))));
        let _ = window;
        Some(div().id("gk").absolute().inset_0().flex().items_center().justify_center().p(px(24.)).bg(hsla(0., 0., 0., 0.6)).occlude()
            .on_click(cx.listener(|this, _, _, cx| this.gk_close(cx)))
            .child(panel).into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn key_names_as_the_page_shows_them() {
        assert_eq!(key_name(""), "Press to set");
        assert_eq!(key_name("KeyV"), "V"); assert_eq!(key_name("Digit5"), "5");
        assert_eq!(key_name("Mouse4"), "Mouse button 4");
        assert_eq!(key_name("ShiftLeft"), "Shift Left"); assert_eq!(key_name("F5"), "F5"); assert_eq!(key_name("ArrowLeft"), "ArrowLeft");
    }
    #[::core::prelude::v1::test]
    fn keys_become_the_codes_the_core_stores() {
        for (k, c) in [("v", "KeyV"), ("5", "Digit5"), ("f5", "F5"), ("space", "Space"), ("`", "Backquote"), ("left", "ArrowLeft"), ("pagedown", "PageDown")] { assert_eq!(code_of(k).as_deref(), Some(c), "{k}"); }
        assert_eq!(code_of("shift"), None);
        assert_eq!(code_of("é"), None);
        for c in ["KeyV", "Digit5", "F5", "Space", "Backquote", "ArrowLeft", "PageDown", "Mouse4"] { assert!(commons_gamekeys::keys::known(c), "{c}"); }
    }
    #[::core::prelude::v1::test]
    fn settings_round_trip_and_refuse_a_game_without_a_key() {
        let mut g = Data::default();
        g.load(&json!({"settings": {"rules": [{"process": null, "key": "F9", "mode": "toggle"}, {"process": "game.exe", "key": "KeyV", "mode": "hold"}], "use_portal": true}, "portal_possible": true, "hold": true, "note": "ok"}));
        assert_eq!(g.any.key, "F9"); assert_eq!(g.rows.len(), 1);
        let s = g.to_settings().unwrap();
        assert_eq!(s["rules"].as_array().unwrap().len(), 2); assert_eq!(s["use_portal"], true);
        let back: commons_gamekeys::Settings = serde_json::from_value(s).unwrap();
        back.check().unwrap();
        g.add_program("GAME.EXE"); assert_eq!(g.rows.len(), 1, "no second entry for the same program");
        g.add_program("other"); assert!(g.to_settings().is_err(), "a game with no key yet");
        g.portal_possible = false; g.rows.pop(); assert_eq!(g.to_settings().unwrap()["use_portal"], false);
    }
}
