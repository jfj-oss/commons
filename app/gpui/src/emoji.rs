//! The emoji picker (for a reaction and for the composer): a search box, the ones used lately, then every emoji by group in a scrolling grid.
//! The web page has only its eight quick reactions (app.js `emojiPop`); this is the whole set, drawn with the system's colour emoji font.
//! The table (emoji_table.rs) holds every single-codepoint emoji with its Unicode name; the search matches the words of the name ("thumbs up",
//! "heart", "face tears"). What was picked last stays first for next time (`gpui-emoji-recent.json` beside the account).
use crate::app::Mega;
use crate::emoji_seq::{SEQ, TONES};
use crate::emoji_table::{EMOJI, GROUPS};
use std::collections::HashMap;
use std::sync::OnceLock;
use crate::look;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

pub const COLS: usize = 8;
pub const CELL: f32 = 36.;
pub const W: f32 = COLS as f32 * CELL + 28.;
pub const H: f32 = 372.;
const RECENT: usize = 24;

#[derive(Clone, Debug, PartialEq)]
pub enum Target { React(String), Composer }

#[derive(Clone, Debug)]
pub enum Row { Head(String), Cells(Vec<String>) }

pub struct EmojiUi {
    pub open: Option<(Target, Point<Pixels>)>,
    pub input: Entity<InputState>,
    pub list: ListState,
    pub rows: Vec<Row>,
    pub recent: Vec<String>,
    pub q: String,
    /// The skin tone applied to everything that takes one: 0 none, 1 light .. 5 dark (kept beside the account).
    pub tone: u8,
}

/// Skin-tone swatches of the picker: a hand in each tone.
pub const SWATCH: [&str; 6] = ["\u{1F44B}", "\u{1F44B}\u{1F3FB}", "\u{1F44B}\u{1F3FC}", "\u{1F44B}\u{1F3FD}", "\u{1F44B}\u{1F3FE}", "\u{1F44B}\u{1F3FF}"];

fn tone_map() -> &'static HashMap<&'static str, &'static [&'static str; 5]> {
    static M: OnceLock<HashMap<&'static str, &'static [&'static str; 5]>> = OnceLock::new();
    M.get_or_init(|| TONES.iter().map(|(k, v)| (*k, v)).collect())
}
/// An emoji in a skin tone (1..=5), if it takes one; else itself.
pub fn toned(e: &str, tone: u8) -> String {
    if tone == 0 || tone > 5 { return e.to_string(); }
    let key: String = e.chars().filter(|c| *c != '\u{FE0F}').collect();
    tone_map().get(key.as_str()).map(|v| v[tone as usize - 1].to_string()).unwrap_or_else(|| e.to_string())
}
fn all() -> impl Iterator<Item = &'static (u8, &'static str, &'static str)> { EMOJI.iter().chain(SEQ.iter()) }

impl EmojiUi {
    pub fn new(window: &mut Window, cx: &mut Context<Mega>, dir: &std::path::Path) -> (EmojiUi, Vec<Subscription>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search emoji"));
        let subs = vec![cx.subscribe_in(&input, window, |this: &mut Mega, _, ev: &InputEvent, window, cx| {
            match ev {
                InputEvent::Change => this.emoji_rebuild(cx),
                InputEvent::PressEnter { .. } => { if let Some(e) = this.emoji.first_hit() { this.emoji_pick(&e, window, cx); } }
                _ => {}
            }
        })];
        let recent: Vec<String> = std::fs::read(dir.join("gpui-emoji-recent.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        (EmojiUi { open: None, input, list: ListState::new(0, ListAlignment::Top, px(300.)), rows: vec![], recent, q: String::new(), tone: std::fs::read(dir.join("gpui-emoji-tone.json")).ok().and_then(|b| serde_json::from_slice::<u8>(&b).ok()).filter(|t| *t <= 5).unwrap_or(0) }, subs)
    }
    fn first_hit(&self) -> Option<String> {
        if self.q.is_empty() { return None; }
        self.rows.iter().find_map(|r| if let Row::Cells(c) = r { c.first().cloned() } else { None })
    }
}

/// The rows of the grid for a query: with none, the recent ones then every group; else the matches of every word of the query.
pub fn rows_for(q: &str, recent: &[String], tone: u8) -> Vec<Row> {
    let chunk = |items: Vec<String>| items.chunks(COLS).map(|c| Row::Cells(c.to_vec())).collect::<Vec<_>>();
    let words: Vec<String> = q.split_whitespace().map(|w| w.to_lowercase()).collect();
    let mut out = vec![];
    if words.is_empty() {
        if !recent.is_empty() { out.push(Row::Head("Recent".into())); out.extend(chunk(recent.to_vec())); }
        for (g, name) in GROUPS.iter().enumerate() {
            out.push(Row::Head((*name).into()));
            out.extend(chunk(all().filter(|e| e.0 as usize == g).map(|e| toned(e.1, tone)).collect()));
        }
    } else {
        let hits: Vec<String> = all().filter(|e| words.iter().all(|w| e.2.contains(w.as_str()) || GROUPS[e.0 as usize].to_lowercase().starts_with(w.as_str()) && w.len() > 3)).map(|e| toned(e.1, tone)).collect();
        if hits.is_empty() { out.push(Row::Head("Nothing matches".into())); } else { out.extend(chunk(hits)); }
    }
    out
}

impl Mega {
    pub fn emoji_open(&mut self, target: Target, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.msg.pop = None;
        self.emoji.open = Some((target, at));
        self.emoji.input.update(cx, |i, cx| { i.set_value("", window, cx); i.focus(window, cx); });
        self.emoji_rebuild(cx);
    }
    pub fn emoji_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.emoji.open.take().is_some() { self.composer.update(cx, |c, cx| c.focus(window, cx)); }
        cx.notify();
    }
    pub fn emoji_rebuild(&mut self, cx: &mut Context<Self>) {
        self.emoji.q = self.emoji.input.read(cx).value().trim().to_string();
        self.emoji.rows = rows_for(&self.emoji.q, &self.emoji.recent, self.emoji.tone);
        self.emoji.list.reset(self.emoji.rows.len());
        cx.notify();
    }
    pub fn emoji_set_tone(&mut self, t: u8, cx: &mut Context<Self>) {
        self.emoji.tone = t.min(5);
        let _ = std::fs::write(self.core.dir.join("gpui-emoji-tone.json"), serde_json::to_vec(&self.emoji.tone).unwrap_or_default());
        self.emoji_rebuild(cx);
    }
    pub fn emoji_pick(&mut self, e: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((target, _)) = self.emoji.open.clone() else { return };
        self.emoji.recent.retain(|x| x != e); self.emoji.recent.insert(0, e.to_string()); self.emoji.recent.truncate(RECENT);
        let _ = std::fs::write(self.core.dir.join("gpui-emoji-recent.json"), serde_json::to_vec(&self.emoji.recent).unwrap_or_default());
        self.emoji.open = None;
        match target {
            Target::React(id) => { self.msg_react(&id, e, window, cx); }
            Target::Composer => {
                let e = e.to_string();
                self.composer.update(cx, |c, cx| { let v = format!("{}{}", c.value(), e); c.set_value(v, window, cx); c.focus(window, cx); });
            }
        }
        cx.notify();
    }

    fn emoji_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        match self.emoji.rows.get(ix) {
            Some(Row::Head(t)) => div().h(px(26.)).px(px(8.)).pt(px(8.)).font_family(look::F_NAME).text_size(px(11.)).text_color(look::mute()).child(t.clone()).into_any_element(),
            Some(Row::Cells(c)) => h_flex().h(px(CELL)).px(px(4.)).children(c.iter().map(|e| {
                let em = e.clone();
                let key = format!("emo-{e}");
                div().id(SharedString::from(key.clone())).relative().flex().items_center().justify_center().size(px(CELL)).rounded(px(10.)).cursor_pointer().text_size(px(22.)).hover(|s| s.bg(hsla(0., 0., 1., 0.12)))
                    .child(e.clone()).child(self.files.hits.track(key)).on_click(cx.listener(move |this, _, window, cx| this.emoji_pick(&em, window, cx)))
            })).into_any_element(),
            None => div().into_any_element(),
        }
    }

    /// The picker over the room, in the coordinates of the message list's area (where the reaction picker lives).
    pub fn emoji_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (target, at) = self.emoji.open.clone()?;
        let area = self.sel.area.get();
        let (aw, ah) = (f32::from(area.size.width), f32::from(area.size.height));
        let (x, y) = match target {
            Target::Composer => ((aw - W - 24.).max(8.), (ah - H - 8.).max(8.)),
            Target::React(_) => {
                let x = (f32::from(at.x - area.left()) + 14. - W).clamp(8., (aw - W - 8.).max(8.));
                let above = f32::from(at.y - area.top()) - H - 8.;
                (x, if above > 8. { above } else { (f32::from(at.y - area.top()) + 12.).min((ah - H - 8.).max(8.)) })
            }
        };
        let list = list(self.emoji.list.clone(), cx.processor(|this, ix: usize, _, cx| this.emoji_row(ix, cx))).size_full();
        let panel = v_flex().id("emoji-panel").absolute().left(px(x)).top(px(y)).w(px(W)).h(px(H)).rounded(px(18.)).bg(rgb(0x1b1922)).border_1().border_color(look::line()).shadow_lg().font_family(look::F_UI).text_size(px(13.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| { if ev.keystroke.key == "escape" { this.emoji_close(window, cx); cx.stop_propagation(); } }))
            .child(h_flex().px(px(12.)).pt(px(10.)).pb(px(6.)).gap(px(6.)).items_center()
                .child(div().flex_1().min_w_0().child(Input::new(&self.emoji.input).appearance(false)))
                .children(SWATCH.iter().enumerate().map(|(i, e)| {
                    let on = self.emoji.tone as usize == i; let key = format!("tone-{i}");
                    div().id(SharedString::from(key.clone())).relative().flex().items_center().justify_center().flex_none().size(px(24.)).rounded_full().cursor_pointer().text_size(px(15.))
                        .when(on, |d| d.bg(hsla(0., 0., 1., 0.18))).hover(|s| s.bg(hsla(0., 0., 1., 0.12)))
                        .child(*e).child(self.files.hits.track(key)).on_click(cx.listener(move |this, _, _, cx| this.emoji_set_tone(i as u8, cx)))
                })))
            .child(div().flex_1().min_h_0().px(px(4.)).pb(px(6.)).child(list));
        Some(div().id("emoji-back").absolute().inset_0().on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.emoji_close(window, cx))).child(panel).into_any_element())
    }

    pub fn emoji_dump(&self) -> serde_json::Value {
        serde_json::json!({"open": self.emoji.open.is_some(), "q": self.emoji.q, "rows": self.emoji.rows.len(), "tone": self.emoji.tone, "recent": self.emoji.recent, "first": self.emoji.rows.iter().find_map(|r| if let Row::Cells(c) = r { c.first().cloned() } else { None })})
    }
}

#[cfg(test)]
mod tests {
    use super::{rows_for, toned, Row};
    use crate::emoji_seq::{SEQ, TONES};
use crate::emoji_table::{EMOJI, GROUPS};
use std::collections::HashMap;
use std::sync::OnceLock;
    #[test]
    fn with_no_query_the_recent_come_first_then_every_group() {
        let rows = rows_for("", &["🥂".to_string()], 0);
        assert!(matches!(&rows[0], Row::Head(t) if t == "Recent"));
        assert!(matches!(&rows[1], Row::Cells(c) if c == &vec!["🥂".to_string()]));
        assert_eq!(rows.iter().filter(|r| matches!(r, Row::Head(_))).count(), 1 + GROUPS.len());
    }
    #[test]
    fn a_query_matches_every_word_of_the_names() {
        let rows = rows_for("thumbs up", &[], 0);
        let all: Vec<String> = rows.iter().filter_map(|r| if let Row::Cells(c) = r { Some(c.clone()) } else { None }).flatten().collect();
        assert!(all.contains(&"👍".to_string()), "{all:?}");
        assert!(all.len() < 6);
    }
    #[test]
    fn heart_and_tears_of_joy_are_found() {
        let flat = |q: &str| rows_for(q, &[], 0).into_iter().filter_map(|r| if let Row::Cells(c) = r { Some(c) } else { None }).flatten().collect::<Vec<_>>();
        assert!(flat("heart").iter().any(|e| e.starts_with('❤')));
        assert!(flat("joy").contains(&"😂".to_string()));
        assert!(flat("zzzzqq").is_empty());
    }
    #[test]
    fn the_table_has_the_web_quick_reactions() {
        for q in ["👍", "🎉", "😮", "🙏", "👀", "🥂", "😂"] { assert!(EMOJI.iter().any(|e| e.1 == q), "{q}"); }
        assert!(EMOJI.iter().any(|e| e.1.starts_with('❤')));
    }
    #[test]
    fn a_tone_changes_hands_and_people_but_not_faces_or_flags() {
        assert_eq!(toned("👍", 3), "👍🏽");
        assert_eq!(toned("👩‍💻", 5), "👩🏿‍💻");
        assert_eq!(toned("😀", 4), "😀");
        assert_eq!(toned("🇯🇵", 2), "🇯🇵");
        assert_eq!(toned("👍", 0), "👍");
        assert_eq!(toned("🏃\u{200d}♂\u{fe0f}", 1), "🏃🏻\u{200d}♂\u{fe0f}");
    }
    #[test]
    fn flags_and_families_are_in_the_picker_and_found_by_name() {
        let flat = |q: &str, t: u8| rows_for(q, &[], t).into_iter().filter_map(|r| if let Row::Cells(c) = r { Some(c) } else { None }).flatten().collect::<Vec<_>>();
        assert!(flat("flag japan", 0).contains(&"🇯🇵".to_string()));
        assert!(flat("family man woman girl", 0).contains(&"👨‍👩‍👧".to_string()));
        assert!(flat("waving hand", 1).contains(&"👋🏻".to_string()));
        assert_eq!(rows_for("", &[], 0).iter().filter(|r| matches!(r, Row::Head(_))).count(), GROUPS.len());
    }
}
