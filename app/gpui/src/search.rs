//! Searching messages (web reference: app/ui/app.js `#search`, `#search-pop`; the core's `search`): a chip in the room's header opens a box and,
//! after a short pause in typing, the matches: where each is (community / room), who wrote it, and its words with the match marked. The core
//! looks through every message stored on this device; "this room" narrows the list to the open room. A click opens the room and goes to the message
//! (the window around it is asked for when it is older than what is held, as the page's `openRoom(gid, id)` does).
use crate::app::Mega;
use crate::icons::{icon, Icon};
use crate::look;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::json;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hit { pub cid: String, pub gid: String, pub id: String, pub community: String, pub channel: String, pub from: String, pub text: String }

pub struct SearchUi {
    pub open: bool,
    pub input: Entity<InputState>,
    /// Only the open room's matches.
    pub here: bool,
    pub hits: Vec<Hit>,
    /// What the list on screen answers to, and whether anything came back at all (for the empty note).
    pub asked: String,
    /// The text changed at this moment and has not been asked yet.
    pub dirty: Option<Instant>,
    pub seq: u64,
}

impl SearchUi {
    pub fn new(window: &mut Window, cx: &mut Context<Mega>) -> (SearchUi, Vec<Subscription>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search messages"));
        let subs = vec![cx.subscribe_in(&input, window, |this: &mut Mega, _, ev: &InputEvent, _, cx| {
            if let InputEvent::Change = ev { this.find.dirty = Some(Instant::now()); cx.notify(); }
        })];
        (SearchUi { open: false, input, here: false, hits: vec![], asked: String::new(), dirty: None, seq: 0 }, subs)
    }
}

/// The web's `plain` for a hit's words, and the places of the query in them (case-insensitive) for the mark.
fn marks(text: &str, q: &str) -> Vec<std::ops::Range<usize>> {
    if q.is_empty() { return vec![]; }
    let (low, ql) = (text.to_lowercase(), q.to_lowercase());
    // lowercasing may change lengths for some letters: only when it does not, the places carry over
    if low.len() != text.len() { return vec![]; }
    low.match_indices(&ql).map(|(i, m)| i..i + m.len()).collect()
}

impl Mega {
    pub fn search_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find.open = true;
        self.find.input.update(cx, |i, cx| i.focus(window, cx));
        cx.notify();
    }
    pub fn search_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find.open = false; self.find.dirty = None;
        self.find.input.update(cx, |i, cx| i.set_value("", window, cx));
        self.find.hits.clear(); self.find.asked.clear();
        self.composer.update(cx, |c, cx| c.focus(window, cx));
        cx.notify();
    }

    /// Every quarter second: a query that has rested for 220 ms is asked (the page's `st_t`); two letters at least.
    pub fn search_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.find.dirty else { return };
        if t.elapsed() < Duration::from_millis(220) { return; }
        self.find.dirty = None;
        let q = self.find.input.read(cx).value().trim().to_string();
        if q.chars().count() < 2 { self.find.hits.clear(); self.find.asked.clear(); cx.notify(); return; }
        self.find.seq += 1;
        let seq = self.find.seq;
        let want = q.clone();
        self.rpc("search", json!({"q": q}), window, cx, move |this, r, _, cx| {
            if seq != this.find.seq { return; }
            let Ok(v) = r else { return };
            this.find.hits = v.as_array().map(|l| l.iter().take(40).map(|x| {
                let s = |k: &str| x[k].as_str().unwrap_or("").to_string();
                crate::search::Hit { cid: s("cid"), gid: s("gid"), id: s("id"), community: s("community"), channel: s("channel"), from: s("from"), text: crate::msgact::plain(&s("text")) }
            }).collect()).unwrap_or_default();
            this.find.asked = want; cx.notify();
        });
    }

    /// Open a hit: its room, then the message in it.
    pub fn search_go(&mut self, h: &Hit, window: &mut Window, cx: &mut Context<Self>) {
        let (cid, gid, id) = (h.cid.clone(), h.gid.clone(), h.id.clone());
        self.search_close(window, cx);
        if self.gid.as_deref() == Some(gid.as_str()) && self.level == crate::app::Level::Chat { self.msg_goto(&id, window, cx); return; }
        self.open_room(cid, gid, window, cx);
        self.msg.around = id.clone(); self.msg.want = Some(id);
        self.win.hash.clear(); self.tick(window, cx);
    }

    pub fn search_chip(&self, cx: &mut Context<Self>) -> AnyElement {
        crate::cards::chip("b-search", "search").on_click(cx.listener(|this, _, window, cx| { if this.find.open { this.search_close(window, cx); } else { this.search_open(window, cx); } })).into_any_element()
    }

    /// The box and its list, over the top right of the room (`#search`, `#search-pop`).
    pub fn search_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.find.open { return None; }
        let q = self.find.input.read(cx).value().trim().to_string();
        let gid = self.gid.clone().unwrap_or_default();
        let hits: Vec<Hit> = self.find.hits.iter().filter(|h| !self.find.here || h.gid == gid).cloned().collect();
        let hi: Hsla = rgb(self.room_duo(&gid).hi).into();
        let font = self.chat_font().0;
        let key = |k: String| self.files.hits.track(k);
        let list = v_flex().id("search-list").w_full().max_h(px(460.)).overflow_y_scroll().p(px(6.)).gap(px(2.))
            .when(hits.is_empty() && self.find.asked == q && !q.is_empty(), |d| d.child(div().p(px(12.)).text_size(px(12.)).text_color(look::mute()).font_family(look::F_NAME).child("Nothing found. Search only covers messages stored on this device.")))
            .children(hits.into_iter().enumerate().map(|(n, h)| {
                let ranges = marks(&h.text, &self.find.asked);
                let st = StyledText::new(h.text.clone()).with_highlights(ranges.into_iter().map(|r| (r, HighlightStyle { background_color: Some(hi.opacity(0.4)), ..Default::default() })));
                let hh = h.clone();
                div().id(SharedString::from(format!("hit-{n}"))).relative().flex().flex_col().gap(px(1.)).w_full().px(px(10.)).py(px(7.)).rounded(px(14.)).cursor_pointer().hover(|s| s.bg(hsla(0., 0., 1., 0.1)))
                    .child(div().font_family(look::F_NAME).text_size(px(11.)).text_color(look::mute()).child(format!("{} / {}", h.community, h.channel)))
                    .child(h_flex().gap(px(6.)).items_baseline().child(div().flex_none().font_family(look::F_NAME).text_size(px(12.)).child(h.from.clone()))
                        .child(div().min_w_0().max_h(px(40.)).overflow_hidden().font_family(font).text_size(px(15.)).line_height(px(20.)).child(st)))
                    .child(key(format!("hit-{n}")))
                    .on_click(cx.listener(move |this, _, window, cx| this.search_go(&hh, window, cx)))
            }));
        Some(v_flex().id("search").absolute().top(px(64.)).right(px(20.)).w(px(460.)).rounded(px(22.)).bg(rgb(0x1b1922)).border_1().border_color(look::line()).shadow_lg().font_family(look::F_UI).text_size(px(13.))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| { if ev.keystroke.key == "escape" { this.search_close(window, cx); cx.stop_propagation(); } }))
            .child(h_flex().items_center().gap(px(8.)).px(px(14.)).pt(px(12.)).pb(px(8.))
                .child(icon(Icon::Search, 16., look::mute()))
                .child(div().flex_1().min_w_0().child(Input::new(&self.find.input).appearance(false)))
                .child(div().id("search-here").relative().px(px(10.)).py(px(4.)).rounded_full().cursor_pointer().text_size(px(12.)).font_family(look::F_NAME)
                    .when(self.find.here, |d| d.bg(hi).text_color(rgb(look::VOID))).when(!self.find.here, |d| d.bg(hsla(0., 0., 1., 0.08)).text_color(look::mute()))
                    .child("this room").child(self.files.hits.track("search-here")).on_click(cx.listener(|this, _, _, cx| { this.find.here = !this.find.here; cx.notify(); })))
                .child(div().id("search-x").relative().flex().items_center().justify_center().size(px(24.)).rounded_full().cursor_pointer().hover(|s| s.bg(hsla(0., 0., 1., 0.12))).child(icon(Icon::Close, 13., look::mute()))
                    .child(self.files.hits.track("search-x")).on_click(cx.listener(|this, _, window, cx| this.search_close(window, cx)))))
            .child(list).into_any_element())
    }

    pub fn search_dump(&self) -> serde_json::Value {
        json!({"open": self.find.open, "asked": self.find.asked, "here": self.find.here, "hits": self.find.hits.iter().map(|h| json!({"gid": h.gid, "id": h.id, "text": h.text, "from": h.from, "room": h.channel})).collect::<Vec<_>>()})
    }
}

#[cfg(test)]
mod tests {
    use super::marks;
    #[test]
    fn the_query_is_marked_wherever_it_is_found_whatever_its_case() { assert_eq!(marks("Hello hello HELLO", "hello"), vec![0..5, 6..11, 12..17]); }
    #[test]
    fn nothing_is_marked_for_an_empty_query() { assert!(marks("abc", "").is_empty()); }
}
