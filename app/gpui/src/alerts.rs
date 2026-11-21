//! What a new message does inside the window, and the per-room rules (app/ui/app.js `news` / `mentionNews` / `refreshMentions`,
//! app/ui/mega-notify.js `soundOk` / the room menu):
//!
//! - **The chime** for something new while the window is in the background or the message is in another room, as the
//!   Settings choice says: every message, only mentions and replies, or nothing. A muted room is silent, a mentions-only room
//!   is silent for plain messages; both stay silent for a mention only when muted.
//! - **The taskbar's ask for attention** (urgency hint on X11, the compositor's "demands attention" on Wayland, the dock
//!   bounce on macOS) for a mention while the window is not the one being looked at.
//! - **The room menu** (right-click or long-press on a room card or its dot): mute for an hour, eight hours, until I unmute,
//!   mentions only, unmute. The rules live in the core (`notify_room`), shared with the system notifications.
//!
//! The system notification itself, and whether one is shown at all, is the core's (crates/core/src/notify.rs, with
//! notifier.rs as the Linux sink); this file does what the web page does around it.
//!
//! Tests only: `MEGA_GPUI_CHIME_FILE` records chimes instead of playing them (chime.rs), `MEGA_GPUI_ATTENTION_FILE` records
//! each ask for attention (the real ask still goes to the test's private window manager).
use crate::app::Mega;
use crate::chime::Chime;
use crate::look;
use crate::prefs::{ReadSize, Sound};
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;

// ------------------------------------------------------------------------------------------------ the rules (pure)

/// What this device does with one room (the core's `Rule`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RoomRule { Muted, Mentions }

/// The room's rule in force at `now` (unix seconds): a mute that has run out is not one. `settings` is `notify_get`'s answer.
pub fn rule(settings: &Value, gid: &str, now: f64) -> Option<RoomRule> {
    let r = settings["rooms"].get(gid)?;
    match r["kind"].as_str()? {
        "muted" => match r["until"].as_f64() { Some(t) if t <= now => None, _ => Some(RoomRule::Muted) },
        "mentions" => Some(RoomRule::Mentions),
        _ => None,
    }
}

/// May a message in this room make a sound? `named`: it mentions or replies to me. (mega-notify.js `soundOk`: notifications have
/// their own global mode; a room's rule applies to both.)
pub fn sound_ok(settings: &Value, gid: &str, named: bool, now: f64) -> bool {
    match rule(settings, gid, now) { None => true, Some(RoomRule::Muted) => false, Some(RoomRule::Mentions) => named }
}

/// The small word a room's card shows when it has a rule.
pub fn mark(settings: &Value, gid: &str, now: f64) -> Option<&'static str> {
    rule(settings, gid, now).map(|r| match r { RoomRule::Muted => "muted", RoomRule::Mentions => "mentions only" })
}

/// How many of the newest `unread` messages of this list are mentions of me or replies to me (app.js `refreshMentions`): my own
/// messages, deleted ones and lines nobody wrote do not count.
pub fn mention_count(list: &[Value], unread: u64, me: &str) -> u64 {
    let (mut left, mut n) = (unread, 0);
    for x in list.iter().rev() {
        if left == 0 { break; }
        if x["mine"].as_bool() == Some(true) || x["deleted"].as_bool() == Some(true) || matches!(x["kind"].as_str(), Some("notice") | Some("member")) { continue; }
        left -= 1;
        let replies_to_me = x["reply"].is_object() && list.iter().any(|y| y["id"] == x["reply"]["id"] && y["mine"].as_bool() == Some(true));
        if crate::arrive::mentions(x["text"].as_str().unwrap_or(""), me) || replies_to_me { n += 1; }
    }
    n
}

/// Which rooms got something new since the last look, and does that call for a chime (`news`).
#[derive(Default)]
pub struct Tracker {
    seen: Option<HashMap<String, u64>>,
    /// Per room: the unread count the mention count was made for, and the mentions among them.
    sig: HashMap<String, u64>,
    pub mention: HashMap<String, u64>,
    seen_mentions: u64,
    busy: bool,
}
impl Tracker {
    /// `now`: unread per room. A chime is due when a room's count went up, the window is away or it is another room, and the
    /// room is not quiet (muted or mentions-only).
    pub fn fresh(&mut self, now: &HashMap<String, u64>, viewing: Option<&str>, away: bool, quiet: impl Fn(&str) -> bool) -> Option<String> {
        let hit = self.seen.as_ref().and_then(|seen| {
            let mut gids: Vec<&String> = now.keys().collect();
            gids.sort();
            gids.into_iter().find(|g| now[*g] > seen.get(*g).copied().unwrap_or(0) && (away || viewing != Some(g.as_str())) && !quiet(g)).cloned()
        });
        self.seen = Some(now.clone());
        hit
    }
    /// Rooms whose mention count is out of date (their unread count changed), and forgets the ones with nothing unread.
    pub fn stale(&mut self, now: &HashMap<String, u64>) -> Vec<(String, u64)> {
        self.sig.retain(|g, _| now.get(g).is_some_and(|n| *n > 0));
        self.mention.retain(|g, _| now.get(g).is_some_and(|n| *n > 0));
        let mut v: Vec<(String, u64)> = now.iter().filter(|(g, n)| **n > 0 && self.sig.get(*g) != Some(*n)).map(|(g, n)| (g.clone(), *n)).collect();
        v.sort();
        v
    }
    pub fn set_mentions(&mut self, gid: &str, unread: u64, n: u64) {
        self.sig.insert(gid.to_string(), unread);
        if n > 0 { self.mention.insert(gid.to_string(), n); } else { self.mention.remove(gid); }
    }
    /// After the counts changed: true when the total of mentions that may be heard went up (`mentionNews`).
    pub fn mentions_rose(&mut self, viewing: Option<&str>, away: bool, ok: impl Fn(&str) -> bool) -> bool {
        let n: u64 = self.mention.iter().filter(|(g, _)| (viewing != Some(g.as_str()) || away) && ok(g)).map(|(_, n)| *n).sum();
        let rose = n > self.seen_mentions;
        self.seen_mentions = n;
        rose
    }
}

// ------------------------------------------------------------------------------------------------ the window's half

/// The state is one per process (there is one window); a static keeps it out of the window's long constructor.
struct State { tracker: Tracker, chime: Option<Chime>, menu: Option<Menu> }
static STATE: std::sync::OnceLock<Mutex<State>> = std::sync::OnceLock::new();
fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.get_or_init(|| Mutex::new(State { tracker: Tracker::default(), chime: None, menu: None })).lock().unwrap_or_else(|p| p.into_inner())
}

#[derive(Clone)]
struct Menu { gid: String, name: String, at: Point<Pixels> }

fn unread_map(sn: &crate::model::Snapshot) -> HashMap<String, u64> {
    let mut m = HashMap::new();
    for c in sn.communities.iter().filter(|c| !c.removed).chain(sn.direct.iter()) {
        for ch in c.channels.iter().filter(|ch| ch.kind != "voice" && ch.joined) { m.insert(ch.gid.clone(), ch.unread); }
    }
    m
}

pub const MENU_W: f32 = 232.;
pub const MENU_ROW: f32 = 34.;
const OPTIONS: [(&str, u64, &str); 4] = [("mute", 3600, "Mute for 1 hour"), ("mute", 28800, "Mute for 8 hours"), ("mute", 0, "Mute until I unmute"), ("mentions", 0, "Mentions only")];

impl Mega {
    fn now_s(&self) -> f64 { crate::ringui::unix() }

    /// After every new snapshot: a chime for something new, and the mention counts that decide the rest.
    pub fn alerts_after_snapshot(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sn) = self.snap.clone() else { return };
        let now_unread = unread_map(&sn);
        let viewing = self.gid.clone().filter(|_| self.level == crate::app::Level::Chat);
        let away = !self.attended(window);
        let (settings, now) = (self.notify_settings.clone(), self.now_s());
        let sound = self.prefs.msg_sound;
        let (fresh, stale) = {
            let mut st = state();
            let f = st.tracker.fresh(&now_unread, viewing.as_deref(), away, |g| !sound_ok(&settings, g, false, now));
            (f, st.tracker.stale(&now_unread))
        };
        if let Some(g) = fresh { if sound == Sound::All { self.chime("message", &g); } }
        if !stale.is_empty() && !std::mem::replace(&mut state().tracker.busy, true) { self.refresh_mentions(stale, sn.me_name.clone(), window, cx); }
        // a count that fell back to nothing changes the mention total too
        self.mention_news(window);
    }

    fn refresh_mentions(&mut self, rooms: Vec<(String, u64)>, me: String, window: &mut Window, cx: &mut Context<Self>) {
        let core = self.core.clone();
        cx.spawn_in(window, async move |this, cx| {
            for (gid, unread) in rooms {
                let g = gid.clone();
                let Ok(r) = core.clone().look(move |app| app.call("messages", json!({"gid": g, "limit": ((unread + 50).div_ceil(50) * 50).max(60)}))).await else { break };
                let n = r.ok().and_then(|v| v["messages"].as_array().map(|l| mention_count(l, unread, &me))).unwrap_or(0);
                state().tracker.set_mentions(&gid, unread, n);
            }
            state().tracker.busy = false;
            let _ = this.update_in(cx, |this, window, cx| { this.mention_news(window); cx.notify(); });
        }).detach();
    }

    /// `mentionNews`: more mentions than before that may be heard: the chime (unless sound is off), and the taskbar when away.
    fn mention_news(&mut self, window: &mut Window) -> bool {
        let viewing = self.gid.clone().filter(|_| self.level == crate::app::Level::Chat);
        let away = !self.attended(window);
        let (settings, now) = (self.notify_settings.clone(), self.now_s());
        let (rose, room) = {
            let mut st = state();
            let rose = st.tracker.mentions_rose(viewing.as_deref(), away, |g| sound_ok(&settings, g, true, now));
            (rose, st.tracker.mention.keys().next().cloned().unwrap_or_default())
        };
        if !rose { return false; }
        if self.prefs.msg_sound != Sound::Off { self.chime("mention", &room); }
        if away { self.attention(window, &room); }
        true
    }

    pub fn chime(&mut self, why: &str, room: &str) {
        let dir = self.core.dir.clone();
        state().chime.get_or_insert_with(|| Chime::new(&dir)).play(why, room);
    }

    /// Asks the taskbar to point at the window.
    pub fn attention(&mut self, window: &mut Window, room: &str) {
        if let Some(f) = std::env::var_os("MEGA_GPUI_ATTENTION_FILE") {
            use std::io::Write;
            if let Ok(mut h) = std::fs::OpenOptions::new().create(true).append(true).open(f) { let _ = writeln!(h, "{}", json!({"attention": true, "room": room})); }
        }
        window.request_attention();
        // Wayland has no urgency hint: an activation request the compositor will not grant (no recent click) is how KWin and
        // Mutter mark a window as demanding attention (gpui's `activate_window` asks for it that way).
        #[cfg(target_os = "linux")]
        if std::env::var_os("WAYLAND_DISPLAY").is_some() { window.activate_window(); }
    }

    // ---- the room menu

    /// Opens the menu for a room at a point of the window (right-click on a card or a dot).
    pub fn room_menu_open(&mut self, gid: &str, name: &str, at: Point<Pixels>, window: &Window, cx: &mut Context<Self>) {
        let vp = window.viewport_size();
        let at = point(at.x.min(vp.width - px(MENU_W + 8.)).max(px(8.)), at.y.min(vp.height - px(MENU_ROW * 7. + 20.)).max(px(8.)));
        state().menu = Some(Menu { gid: gid.to_string(), name: name.to_string(), at });
        cx.notify();
    }
    pub fn room_menu_close(&mut self, cx: &mut Context<Self>) { if state().menu.take().is_some() { cx.notify(); } }
    pub fn room_menu_open_gid(&self) -> Option<String> { state().menu.as_ref().map(|m| m.gid.clone()) }
    /// What a row of the menu does: `kind` is mute | mentions | none (the web's `data-kind`), `secs` how long a mute lasts (0: until unmuted).
    pub fn room_menu_pick(&mut self, kind: &str, secs: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = state().menu.take() else { return };
        self.set_setting("notify_room", json!({"gid": m.gid, "kind": kind, "secs": secs}), window, cx);
    }

    /// The menu, drawn over everything while it is open.
    pub fn room_menu_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let m = state().menu.clone()?;
        let cur = rule(&self.notify_settings, &m.gid, self.now_s());
        let until = self.notify_settings["rooms"][m.gid.as_str()]["until"].as_f64();
        let cur_key = match (cur, until) { (Some(RoomRule::Mentions), _) => "mentions", (Some(RoomRule::Muted), None) => "forever", _ => "" };
        let key_of = |kind: &str, secs: u64| if kind == "mentions" { "mentions" } else if secs == 0 { "forever" } else { "" };
        let row = |id: (&'static str, usize), text: String, on: bool| div().id(id).h(px(MENU_ROW)).px(px(14.)).flex().items_center().gap(px(8.)).cursor_pointer().hover(|d| d.bg(hsla(0., 0., 1., 0.07)))
            .child(div().w(px(12.)).flex_none().text_color(look::paper()).child(if on { "\u{2713}" } else { "" })).child(text);
        let mut col = v_flex().id("room-menu").absolute().left(m.at.x).top(m.at.y).w(px(MENU_W)).py(px(4.)).rounded(px(10.)).bg(rgb(0x1b1922)).border_1().border_color(look::line()).shadow_lg().font_family(look::F_UI).text_size(px(13.))
            .occlude()
            .child(div().px(px(14.)).py(px(6.)).font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).overflow_hidden().text_ellipsis().whitespace_nowrap().child(m.name.clone()));
        for (i, (kind, secs, label)) in OPTIONS.iter().enumerate() {
            let (kind, secs) = (*kind, *secs);
            col = col.child(row(("rm-opt", i), label.to_string(), cur_key == key_of(kind, secs) && !cur_key.is_empty())
                .on_click(cx.listener(move |this, _, window, cx| this.room_menu_pick(kind, secs, window, cx))));
        }
        if cur.is_some() {
            col = col.child(row(("rm-opt", 9), "Unmute".into(), false).on_click(cx.listener(|this, _, window, cx| this.room_menu_pick("none", 0, window, cx))));
            if let (Some(RoomRule::Muted), Some(t)) = (cur, until) {
                use chrono::TimeZone as _;
                if let Some(d) = chrono::Local.timestamp_opt(t as i64, 0).single() { col = col.child(div().px(px(14.)).py(px(6.)).text_size(px(12.)).text_color(look::mute()).child(format!("Muted until {}", d.format("%H:%M")))); }
            }
        }
        // a click anywhere else closes it
        Some(div().id("room-menu-shade").absolute().inset_0().occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.room_menu_close(cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| this.room_menu_close(cx)))
            .child(col).into_any_element())
    }

    /// The word a room card shows after its line ("muted", "mentions only"), or nothing.
    pub fn room_mark(&self, gid: &str) -> Option<&'static str> { mark(&self.notify_settings, gid, self.now_s()) }

    /// For the test port: what the window holds about alerts.
    pub fn alerts_dump(&self) -> Value {
        let st = state();
        json!({"mentions": st.tracker.mention, "menu": st.menu.as_ref().map(|m| json!({"gid": m.gid, "name": m.name})),
               "sound": format!("{:?}", self.prefs.msg_sound), "read_size": format!("{:?}", self.prefs.read_size), "settings": self.notify_settings})
    }
}


thread_local! { static DL_INPUT: std::cell::RefCell<Option<Entity<InputState>>> = const { std::cell::RefCell::new(None) }; }

impl Mega {
    /// The settings rows this file owns (the web's "alerts and sound", reading size and "large files" sections, app/ui/mega.js
    /// `optsHTML`): the sound for new messages, how big the chat text reads, and the folder large downloads are written to.
    pub fn alerts_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let label = |t: &str| div().mt_4().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child(t.to_string());
        let note = |t: &str| div().text_size(px(13.)).text_color(look::mute()).child(t.to_string());
        fn choice(id: &'static str, on: bool, text: &'static str) -> Stateful<Div> {
            div().id(id).relative().px_3().py_1().border_1().border_color(hsla(0., 0., 0., 0.)).cursor_pointer()
                .when(on, |d| d.text_color(rgb(0x0c0d12)).child(crate::squircle::back(999., look::paper())))
                .child(div().relative().child(text))
                .child(crate::squircle::front(999., None, Some((1., if on { look::paper() } else { look::line() })), None))
        }
        let dl = DL_INPUT.with(|d| {
            let mut d = d.borrow_mut();
            if d.is_none() {
                let start = self.snap.as_ref().map(|s| s.download_dir.clone()).unwrap_or_default();
                let i = cx.new(|cx| InputState::new(window, cx).placeholder("your Downloads folder"));
                i.update(cx, |i, cx| i.set_value(start, window, cx));
                *d = Some(i);
            }
            d.clone().unwrap()
        });
        let (sound, size) = (self.prefs.msg_sound, self.prefs.read_size);
        let set_sound = |v: Sound| cx.listener(move |this: &mut Self, _: &ClickEvent, _, cx| { this.prefs.msg_sound = v; this.prefs.save(); cx.notify(); });
        let set_size = |v: ReadSize| cx.listener(move |this: &mut Self, _: &ClickEvent, _, cx| { this.prefs.read_size = v; this.prefs.save(); this.list.remeasure(); this.reading.full.clear(); cx.notify(); });
        let dl2 = dl.clone();
        v_flex().gap_2()
            .child(label("sound for new messages"))
            .child(h_flex().gap_2()
                .child(choice("snd-all", sound == Sound::All, "All messages").on_click(set_sound(Sound::All)))
                .child(choice("snd-men", sound == Sound::Mentions, "Mentions and replies").on_click(set_sound(Sound::Mentions)))
                .child(choice("snd-off", sound == Sound::Off, "Off").on_click(set_sound(Sound::Off))))
            .child(note("Plays when mega is in the background or the message is in another room. A room you mute is silent here too. Mute a room by right-clicking its card."))
            .child(label("reading size"))
            .child(h_flex().gap_2()
                .child(choice("rs-s", size == ReadSize::Small, "Small").on_click(set_size(ReadSize::Small)))
                .child(choice("rs-n", size == ReadSize::Normal, "Normal").on_click(set_size(ReadSize::Normal)))
                .child(choice("rs-l", size == ReadSize::Large, "Large").on_click(set_size(ReadSize::Large))))
            .child(label("large-file downloads folder"))
            .child(h_flex().gap_2().child(div().w(px(360.)).child(Input::new(&dl)))
                .child(Button::new("save-dl").label("Save").on_click(cx.listener(move |this, _, window, cx| { let d = dl2.read(cx).value().to_string(); this.set_setting("set_download_dir", json!({"dir": d}), window, cx); }))))
            .child(note("Written here as they arrive, one verified piece at a time. Leave empty for your usual Downloads folder. Each download still asks where to save it."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(rooms: Value) -> Value { json!({"mode": "all", "show_text": false, "rooms": rooms}) }
    fn unread(l: &[(&str, u64)]) -> HashMap<String, u64> { l.iter().map(|(g, n)| (g.to_string(), *n)).collect() }

    #[::core::prelude::v1::test]
    fn a_mute_silences_everything_and_mentions_only_lets_the_named_through() {
        let st = s(json!({"m": {"kind": "muted", "until": null}, "t": {"kind": "muted", "until": 150.}, "o": {"kind": "mentions"}}));
        assert!(!sound_ok(&st, "m", false, 100.) && !sound_ok(&st, "m", true, 100.));
        assert!(!sound_ok(&st, "t", true, 100.), "a timed mute is on");
        assert!(sound_ok(&st, "t", false, 150.), "and ends at its time");
        assert!(!sound_ok(&st, "o", false, 100.) && sound_ok(&st, "o", true, 100.));
        assert!(sound_ok(&st, "free", false, 100.));
        assert_eq!((mark(&st, "m", 1.), mark(&st, "o", 1.), mark(&st, "free", 1.), mark(&st, "t", 200.)), (Some("muted"), Some("mentions only"), None, None));
    }

    #[::core::prelude::v1::test]
    fn the_newest_unread_messages_are_searched_for_mentions_and_replies() {
        let list = vec![
            json!({"id": "1", "text": "hello", "mine": true}),
            json!({"id": "2", "text": "old news for @Nat", "mine": false}),
            json!({"id": "3", "text": "hi @Nat", "mine": false}),
            json!({"id": "4", "text": "nothing", "mine": false}),
            json!({"id": "5", "text": "yes", "mine": false, "reply": {"id": "1"}}),
            json!({"id": "6", "text": "@Nat gone", "mine": false, "deleted": true}),
            json!({"id": "7", "text": "Ben joined @Nat", "kind": "member"}),
        ];
        assert_eq!(mention_count(&list, 2, "Nat"), 1, "the newest two are 5 (a reply to me) and 4");
        assert_eq!(mention_count(&list, 3, "Nat"), 2, "...and 3 mentions me (6 and 7 never count)");
        assert_eq!(mention_count(&list, 4, "Nat"), 3, "...and 2 too");
        assert_eq!(mention_count(&list, 0, "Nat"), 0);
        assert_eq!(mention_count(&list, 9, "Nat"), 3, "my own message never counts");
    }

    #[::core::prelude::v1::test]
    fn a_chime_is_due_for_a_new_message_somewhere_you_are_not_looking() {
        let mut t = Tracker::default();
        let quiet = |g: &str| g == "q";
        // the first look only learns the counts
        assert_eq!(t.fresh(&unread(&[("a", 0), ("b", 2)]), Some("a"), false, quiet), None);
        assert_eq!(t.fresh(&unread(&[("a", 0), ("b", 2)]), Some("a"), false, quiet), None, "nothing changed");
        assert_eq!(t.fresh(&unread(&[("a", 0), ("b", 3)]), Some("a"), false, quiet).as_deref(), Some("b"));
        // the room I am reading chimes only when the window is away
        assert_eq!(t.fresh(&unread(&[("a", 1), ("b", 3)]), Some("a"), false, quiet), None);
        assert_eq!(t.fresh(&unread(&[("a", 2), ("b", 3)]), Some("a"), true, quiet).as_deref(), Some("a"));
        // a quiet (muted or mentions-only) room never does
        assert_eq!(t.fresh(&unread(&[("a", 2), ("b", 3), ("q", 1)]), Some("a"), true, quiet), None);
        // counts going down is not news
        assert_eq!(t.fresh(&unread(&[("a", 0), ("b", 0), ("q", 1)]), None, true, quiet), None);
    }

    #[::core::prelude::v1::test]
    fn mention_counts_are_made_once_per_change_and_a_rise_is_news() {
        let mut t = Tracker::default();
        let now = unread(&[("a", 2), ("b", 0)]);
        assert_eq!(t.stale(&now), vec![("a".to_string(), 2)]);
        t.set_mentions("a", 2, 1);
        assert!(t.stale(&now).is_empty(), "same count, nothing to look at again");
        assert!(t.mentions_rose(None, false, |_| true));
        assert!(!t.mentions_rose(None, false, |_| true), "the same total is not news twice");
        // the room being read does not count while the window is looked at, and does when it is away
        let mut t = Tracker::default();
        t.set_mentions("a", 1, 1);
        assert!(!t.mentions_rose(Some("a"), false, |_| true));
        assert!(t.mentions_rose(Some("a"), true, |_| true));
        // a muted room's mention is not heard
        let mut t = Tracker::default();
        t.set_mentions("m", 1, 1);
        assert!(!t.mentions_rose(None, true, |g| g != "m"));
        // all read: forgotten
        assert_eq!(t.stale(&unread(&[("m", 0)])), vec![]);
        assert!(t.mention.is_empty());
    }
}
