//! What a person does to a message: reply, edit, delete and react. Web reference: app/ui/app.js (`msgRow` actions, `emojiPop`, `startEdit`,
//! `renderComposer`, `send`, the click handler of `#stream`), mega-touch.js (`menuFor`, the menu of a long press), mega-polish.css.
//! Everything is the core's own call (`send_text` with `reply`, `edit`, `delete`, `file_delete`, `react`): the core refuses what a role may not do,
//! the buttons follow the same rules so a person is not offered what will fail (own messages; an admin or the owner may delete anyone's).
//!   * a row shows four small tools while the pointer is over it (reply, react, edit for your own text, delete for your own or as an admin);
//!   * the right-click menu offers the same with the quick reactions on top, and Copy;
//!   * a reaction chip toggles your reaction; the smile chip after the chips opens the picker;
//!   * a quote above a reply jumps to the message it answers (flashing it), loading older messages first when it is not in the window.
use crate::app::Mega;
use crate::icons::{icon, Icon};
use crate::look;
use crate::model::Reply;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::h_flex;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::json;

/// The web's quick reactions (app.js `QUICK`).
pub const QUICK: [&str; 8] = ["👍", "❤️", "😂", "🎉", "😮", "🙏", "👀", "🥂"];
/// How long a jumped-to message stays lit (mega.css `.flash`, 1700 ms).
pub const FLASH: f64 = 1.7;

#[derive(Default)]
pub struct MsgUi {
    /// The message the composer answers, or the message it edits.
    pub reply: Option<Reply>,
    pub edit: Option<String>,
    /// The reaction picker: where it was opened (window pixels) and the message it is for.
    pub pop: Option<(Point<Pixels>, String)>,
    /// The message just jumped to and when (seconds on the window's clock).
    pub flash: Option<(String, f64)>,
    /// Kept across a longer window so the room does not move under the reader: the message at the top of the view and how far its top is above the view's top.
    pub keep: Option<(String, Pixels)>,
    /// A jump to a message that is not in the window yet: ask the core for the window around it; and the message to flash once it is there.
    pub around: String,
    pub want: Option<String>,
    /// Messages that arrived while the reader was away from the end (the jump button's count).
    pub below: usize,
    /// The core's window did not hold the message that was asked for: say so once.
    pub say_gone: bool,
    /// The names of the room's people, asked for every few seconds while the room is open.
    pub names_at: Option<std::time::Instant>,
    pub names_cid: String,
}

impl ActCtx { pub fn can_act(&self) -> bool { true } }

/// What a row needs to show its tools and answer clicks, copied out of the window's state.
#[derive(Clone)]
pub struct ActCtx {
    pub me: WeakEntity<Mega>,
    pub hits: crate::calldock::Hits,
    pub can_edit: bool, pub can_delete: bool,
    /// 1 at the start of a flash, 0 when it has faded.
    pub flash: f32,
    /// This message is addressed to me (my @name, or a reply to my message).
    pub mention: bool,
    /// The message this one answers is deleted: no quote.
    pub quote_gone: bool,
    /// The first row of a window that has older messages before it.
    pub older: bool,
    pub hi: Hsla,
}

pub fn on(me: &WeakEntity<Mega>, f: impl Fn(&mut Mega, &mut Window, &mut Context<Mega>) + 'static) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let me = me.clone();
    move |_, window, cx| { let _ = me.update(cx, |this, cx| f(this, window, cx)); }
}
/// The same, with the click (for where it happened).
pub fn on_at(me: &WeakEntity<Mega>, f: impl Fn(&mut Mega, Point<Pixels>, &mut Window, &mut Context<Mega>) + 'static) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let me = me.clone();
    move |ev, window, cx| { let p = ev.mouse_position().unwrap_or_default(); let _ = me.update(cx, |this, cx| f(this, p, window, cx)); }
}

/// The web's `plain`: a message's words without its `**`, `*` and backticks (quotes and the reply bar show it so).
pub fn plain(t: &str) -> String {
    let mut out = String::new();
    for (i, seg) in t.split('`').enumerate() { let _ = i; out.push_str(seg); }
    out.replace("**", "")
}

/// One small button of the tools over a row.
fn tool(key: String, ic: Icon, tip: &'static str, c: &ActCtx, f: impl Fn(&mut Mega, Point<Pixels>, &mut Window, &mut Context<Mega>) + 'static) -> impl IntoElement {
    div().id(SharedString::from(key.clone())).relative().flex().items_center().justify_center().size(px(26.)).rounded(px(10.)).cursor_pointer()
        .hover(|s| s.bg(hsla(0., 0., 1., 0.12)))
        .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
        .child(icon(ic, 17., hsla(0., 0., 0.94, 0.72))).child(c.hits.track(key))
        .on_click(on_at(&c.me, f))
}

/// The tools over a row (app.js `.actions`): reply, react, edit, delete; shown only while the pointer is over the row.
pub fn tools(m: &crate::model::Msg, c: &ActCtx, group: SharedString) -> impl IntoElement {
    let id = m.id.clone();
    let (a, b, d, e) = (id.clone(), id.clone(), id.clone(), id.clone());
    h_flex().absolute().top(px(0.)).right(px(18.)).gap(px(1.)).p(px(2.)).rounded(px(14.)).bg(rgb(0x1b1922)).border_1().border_color(look::line()).shadow_lg()
        .opacity(0.).group_hover(group, |s| s.opacity(1.))
        .child(tool(format!("act-reply-{id}"), Icon::Reply, "Reply", c, move |this, _, window, cx| this.msg_reply(&a, window, cx)))
        .child(tool(format!("act-react-{id}"), Icon::Smile, "React", c, move |this, p, _, cx| { this.msg.pop = Some((p, b.clone())); cx.notify(); }))
        .when(c.can_edit, |d_| d_.child(tool(format!("act-edit-{id}"), Icon::Pencil, "Edit", c, move |this, _, window, cx| this.msg_edit(&d, window, cx))))
        .when(c.can_delete, |d_| d_.child(tool(format!("act-del-{id}"), Icon::Trash, "Delete", c, move |this, _, window, cx| this.msg_delete(&e, window, cx))))
}

/// A reaction chip (app.js `.react`): the emoji and its count; yours is filled with the room's bright colour. Click toggles yours.
pub fn chip(m: &crate::model::Msg, r: &crate::model::Reaction, shown: u64, k: f32, c: &ActCtx) -> Stateful<Div> {
    let (id, em) = (m.id.clone(), r.emoji.clone());
    let key = format!("react-{}-{}", m.id, r.emoji);
    let tip: SharedString = r.names.join(", ").into();
    div().id(SharedString::from(key.clone())).relative().flex().items_center().gap(px(4.)).h(px(24. * k)).px(px(8. * k)).rounded_full().cursor_pointer()
        .text_size(px(13. * k)).when(!r.mine, |d| d.bg(hsla(0., 0., 1., 0.08)).hover(|s| s.bg(hsla(0., 0., 1., 0.16)))).when(r.mine, |d| d.bg(c.hi).text_color(rgb(look::VOID)))
        .when(!tip.is_empty(), |d| d.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx)))
        .child(r.emoji.clone()).child(div().font_family(look::F_MONO).text_size(px(12. * k)).child(shown.to_string())).child(c.hits.track(key))
        .on_click(on(&c.me, move |this, window, cx| this.msg_react(&id, &em, window, cx)))
}

/// The smile chip after a message's reactions: opens the picker.
pub fn add_chip(m: &crate::model::Msg, k: f32, c: &ActCtx) -> impl IntoElement {
    let id = m.id.clone();
    let key = format!("react-add-{}", m.id);
    div().id(SharedString::from(key.clone())).relative().flex().items_center().justify_center().h(px(24. * k)).w(px(28. * k)).rounded_full().cursor_pointer()
        .hover(|s| s.bg(hsla(0., 0., 1., 0.1))).tooltip(|window, cx| Tooltip::new("Add a reaction").build(window, cx))
        .child(icon(Icon::Smile, 15. * k, look::mute())).child(c.hits.track(key))
        .on_click(on_at(&c.me, move |this, p, _, cx| { this.msg.pop = Some((p, id.clone())); cx.notify(); }))
}

/// The quote above a reply (app.js `.quote`): who and what it answers; click goes to it.
pub fn quote(r: &Reply, font: &'static str, size: f32, c: &ActCtx) -> impl IntoElement {
    let id = r.id.clone();
    let key = format!("quote-{}", r.id);
    h_flex().id(SharedString::from(key.clone())).relative().max_w_full().gap(px(6.)).pl_2().border_l_2().border_color(look::line()).hover(|s| s.border_color(look::paper())).cursor_pointer()
        .text_color(look::mute()).items_baseline()
        .tooltip(|window, cx| Tooltip::new("Go to this message").build(window, cx))
        .child(div().flex_none().font_family(look::F_NAME).text_size(px(12.)).text_color(look::paper()).child(r.name.clone()))
        .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().font_family(font).text_size(px(size)).opacity(0.8).child(plain(&r.text)))
        .child(c.hits.track(key))
        .on_click(on(&c.me, move |this, window, cx| this.msg_goto(&id, window, cx)))
}

impl Mega {
    /// May I delete this message here? My own, or anyone's as an admin or the owner (mega-touch.js `admin()`).
    pub fn is_admin_here(&self) -> bool {
        self.gid.as_deref().and_then(|g| self.channel(g)).is_some_and(|(c, _)| !c.direct && (c.role == "owner" || c.role == "admin"))
    }
    pub fn msg_of(&self, id: &str) -> Option<&crate::model::Msg> { self.win.msgs.iter().find(|m| m.id == id) }

    pub fn msg_react(&mut self, id: &str, emoji: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(gid) = self.gid.clone() else { return };
        let mine = self.msg_of(id).is_some_and(|m| m.reactions.iter().any(|r| r.emoji == emoji && r.mine));
        self.msg.pop = None;
        self.rpc("react", json!({"gid": gid, "id": id, "emoji": emoji, "on": !mine}), window, cx, |this, r, window, cx| {
            if let Err(e) = r { this.say(e, cx); }
            this.snap_hash.clear(); this.tick(window, cx);
        });
        cx.notify();
    }

    pub fn msg_reply(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.msg_of(id) else { return };
        let text = if m.text.is_empty() { m.file.as_ref().map(|f| format!("[file] {}", f.name)).unwrap_or_default() } else { m.text.clone() };
        self.msg.reply = Some(Reply { id: m.id.clone(), name: m.name.clone(), text: text.chars().take(120).collect() });
        if self.msg.edit.take().is_some() { self.composer.update(cx, |c, cx| c.set_value("", window, cx)); }
        self.composer.update(cx, |c, cx| c.focus(window, cx));
        cx.notify();
    }

    pub fn msg_edit(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.msg_of(id) else { return };
        if !m.mine || m.kind != "text" { return; }
        let text = m.text.clone();
        self.msg.edit = Some(id.to_string()); self.msg.reply = None;
        self.composer.update(cx, |c, cx| { c.set_value(text, window, cx); c.focus(window, cx); });
        cx.notify();
    }

    pub fn msg_cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.msg.edit.take().is_some() { self.composer.update(cx, |c, cx| c.set_value("", window, cx)); }
        self.msg.reply = None;
        cx.notify();
    }

    pub fn msg_delete(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(gid) = self.gid.clone() else { return };
        let large = self.msg_of(id).is_some_and(|m| m.file.as_ref().is_some_and(|f| f.large.is_some()));
        if self.msg.edit.as_deref() == Some(id) { self.msg_cancel(window, cx); }
        if self.msg.reply.as_ref().is_some_and(|r| r.id == id) { self.msg.reply = None; }
        self.rpc(if large { "file_delete" } else { "delete" }, json!({"gid": gid, "id": id}), window, cx, |this, r, window, cx| {
            if let Err(e) = r { this.say(e, cx); }
            this.tick(window, cx);
        });
    }

    /// Escape cancels a reply or an edit; Up in an empty box edits my last text message (app.js `#input` keydown).
    pub fn msg_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let k = ev.keystroke.key.as_str();
        if self.msg.pop.is_some() && k == "escape" { self.msg.pop = None; cx.notify(); return true; }
        let in_box = { let fh = self.composer.focus_handle(cx); fh.is_focused(window) || fh.contains_focused(window, cx) };
        if !in_box { return false; }
        if k == "escape" && (self.msg.reply.is_some() || self.msg.edit.is_some()) { self.msg_cancel(window, cx); return true; }
        if k == "up" && !ev.keystroke.modifiers.shift && self.composer.read(cx).value().is_empty() && self.msg.edit.is_none() {
            if let Some(id) = self.win.msgs.iter().rev().find(|m| m.mine && m.kind == "text" && !m.deleted).map(|m| m.id.clone()) { self.msg_edit(&id, window, cx); return true; }
        }
        false
    }

    /// Jump to a message: lit and centred when it is in the window, else the window around it is asked for first.
    pub fn msg_goto(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.win.msgs.iter().position(|m| m.id == id) {
            self.list.scroll_to(ListOffset { item_ix: ix.saturating_sub(1), offset_in_item: px(0.) });
            self.msg.flash = Some((id.to_string(), self.now_secs()));
            self.msg.want = None;
            window.request_animation_frame();
            cx.notify();
            return;
        }
        if self.msg.want.as_deref() == Some(id) { self.msg.want = None; self.say("That message is not on this device.", cx); return; }
        self.msg.around = id.to_string(); self.msg.want = Some(id.to_string());
        self.win.hash.clear();
        self.tick(window, cx);
    }

    /// "Show earlier messages": 200 more, the reader's place kept (app.js `a === 'older'`).
    pub fn msg_older(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let top = self.list.logical_scroll_top();
        if let Some(m) = self.win.msgs.get(top.item_ix) { self.msg.keep = Some((m.id.clone(), top.offset_in_item)); }
        self.limit += 200;
        self.win.hash.clear();
        self.tick(window, cx);
    }

    /// How lit a message still is (1 down to 0 over `FLASH` seconds).
    pub fn flash_of(&self, id: &str, now: f64) -> f32 {
        match &self.msg.flash { Some((f, t0)) if f == id && now - t0 < FLASH => (1. - ((now - t0) / FLASH) as f32).clamp(0., 1.), _ => 0. }
    }

    /// The web's `refreshMembers`: the people of the open community, for @mentions in the words.
    pub fn msg_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cid) = self.cid.clone().filter(|_| self.level == crate::app::Level::Chat && self.gid.is_some()) else { return };
        if self.msg.names_cid == cid && self.msg.names_at.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(4)) { return; }
        self.msg.names_at = Some(std::time::Instant::now()); self.msg.names_cid = cid.clone();
        self.rpc("members", json!({"cid": cid}), window, cx, move |this, r, _, cx| {
            let Ok(v) = r else { return };
            if this.cid.as_deref() != Some(cid.as_str()) { return; }
            let names: Vec<String> = v.as_array().map(|l| l.iter().filter_map(|m| m["name"].as_str().map(String::from)).collect()).unwrap_or_default();
            let me = this.snap.as_ref().map(|s| s.me_name.clone()).unwrap_or_default();
            if names != this.win.names || me != this.win.me { this.win.names = names; this.win.me = me; this.win.mark_all(); cx.notify(); }
        });
    }

    /// A web address in a message: the core checks it and opens it; when no browser starts the address is copied (app.js `openLink`).
    pub fn open_link(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        let u = url.to_string();
        self.rpc("open_link", json!({"url": url}), window, cx, move |this, r, _, cx| {
            match r {
                Ok(v) if v["opened"].as_bool() == Some(true) => {}
                Ok(_) => { cx.write_to_clipboard(ClipboardItem::new_string(u)); this.say("Could not start a browser; the link is copied", cx); }
                Err(e) => this.say(e, cx),
            }
        });
    }

    // ---- the reply and edit bar over the composer ----
    pub fn reply_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (ic, body) = if let Some(r) = &self.msg.reply {
            (Icon::Reply, h_flex().flex_1().min_w_0().gap(px(6.)).items_baseline().overflow_hidden()
                .child(div().flex_none().child("Replying to")).child(div().flex_none().text_color(look::paper()).child(r.name.clone()))
                .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().font_family(self.chat_font().0).text_size(px(15.)).child(plain(&r.text))).into_any_element())
        } else if self.msg.edit.is_some() {
            (Icon::Pencil, div().flex_1().child("Editing your message").into_any_element())
        } else { return None };
        Some(h_flex().id("replybar").relative().w_full().items_center().gap(px(8.)).px(px(10.)).py(px(5.)).mb(px(2.)).rounded(px(10.)).bg(hsla(0., 0., 1., 0.05)).font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute())
            .child(icon(ic, 15., rgb(self.room_duo(self.gid.as_deref().unwrap_or("")).hi).into()))
            .child(body)
            .child(div().id("reply-cancel").relative().flex().items_center().justify_center().size(px(22.)).rounded_full().cursor_pointer().hover(|s| s.bg(hsla(0., 0., 1., 0.12)))
                .tooltip(|window, cx| Tooltip::new("Cancel").build(window, cx)).child(icon(Icon::Close, 13., look::mute())).child(self.files.hits.track("reply-cancel"))
                .on_click(cx.listener(|this, _, window, cx| this.msg_cancel(window, cx))))
            .into_any_element())
    }

    // ---- the reaction picker ----
    /// The picker over the room: the quick reactions as the web has them (and a way to the whole set).
    pub fn react_pop(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (at, id) = self.msg.pop.clone()?;
        let area = self.sel.area.get();
        let m = self.msg_of(&id)?;
        let w = 9. * 34. + 14.;
        let x = (f32::from(at.x - area.left()) + 14. - w).clamp(8., (f32::from(area.size.width) - w - 8.).max(8.));
        let above = f32::from(at.y - area.top()) - 52.;
        let y = if above > 8. { above } else { f32::from(at.y - area.top()) + 12. };
        let row = h_flex().id("react-pop").absolute().left(px(x)).top(px(y)).p(px(6.)).gap(px(2.)).rounded(px(18.)).bg(rgb(0x1b1922)).border_1().border_color(look::line()).shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(QUICK.iter().map(|em| {
                let on_ = m.reactions.iter().any(|r| r.emoji == *em && r.mine);
                let (mid, e) = (id.clone(), em.to_string());
                let key = format!("pop-{em}");
                div().id(SharedString::from(key.clone())).relative().flex().items_center().justify_center().size(px(32.)).rounded(px(12.)).cursor_pointer().text_size(px(20.)).when(on_, |d| d.bg(hsla(0., 0., 1., 0.16)))
                    .hover(|s| s.bg(hsla(0., 0., 1., 0.1))).child(em.to_string()).child(self.files.hits.track(key))
                    .on_click(cx.listener(move |this, _, window, cx| this.msg_react(&mid, &e, window, cx)))
            }))
            .child({
                let mid = id.clone();
                div().id("pop-more").relative().flex().items_center().justify_center().size(px(32.)).rounded(px(12.)).cursor_pointer().hover(|s| s.bg(hsla(0., 0., 1., 0.1)))
                    .tooltip(|window, cx| Tooltip::new("More emoji").build(window, cx)).child(icon(Icon::Plus, 16., look::mute())).child(self.files.hits.track("pop-more"))
                    .on_click(on_at(&self.files.me.clone().unwrap(), move |this, p, window, cx| this.emoji_open(crate::emoji::Target::React(mid.clone()), p, window, cx)))
            });
        Some(div().absolute().inset_0().id("react-back").on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| { this.msg.pop = None; cx.notify(); }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| { this.msg.pop = None; cx.notify(); }))
            .child(row).into_any_element())
    }

    /// For the test port: what the composer is doing and where the tools are.
    pub fn msg_dump(&self) -> serde_json::Value {
        json!({"reply": self.msg.reply.as_ref().map(|r| json!({"id": r.id, "name": r.name, "text": r.text})), "edit": self.msg.edit, "pop": self.msg.pop.as_ref().map(|(_, id)| id), "flash": self.msg.flash.as_ref().map(|(id, _)| id),
            "below": self.msg.below, "msgs": self.win.msgs.iter().rev().take(40).map(|m| json!({"id": m.id, "text": m.shown(), "raw": m.text, "mine": m.mine, "edited": m.edited, "deleted": m.deleted, "reply": m.reply.as_ref().map(|r| r.id.clone()), "reactions": m.reactions.iter().map(|r| json!({"emoji": r.emoji, "count": r.count, "mine": r.mine})).collect::<Vec<_>>(), "spans": m.rich.spans.len()})).collect::<Vec<_>>(), "first": self.win.msgs.first().map(|m| m.id.clone()), "limit": self.limit, "more": self.win.more, "hits": self.files.hits.dump()})
    }
}
