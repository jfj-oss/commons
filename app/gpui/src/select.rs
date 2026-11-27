//! Selecting and copying message text (the web gets this from the browser): drag over a message or across several, double-click a
//! word, triple-click a message, Ctrl+C, Ctrl+A, and a small right-click menu with Copy.
//!
//! The top half of this file is the maths only (no GPUI) so it can be tested: where the selection runs from and to, what part of
//! one row it covers, what gets copied, which row a pointer belongs to. The bottom half is the mouse and key handling over the
//! text laid out by the rows (views.rs `row`): each row's body is a `StyledText`, and once it has been painted its `TextLayout` and
//! bounds are kept in `Sel::texts`, so a pointer can be turned into a character offset exactly where the text is drawn, in a row
//! drawn smaller by the reading zone as in any other (the smaller row's text is laid out smaller; there is nothing to convert).
use crate::app::Mega;
use gpui_kit::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

/// A place in the open room's text: the message's row and a byte offset into its text.
pub type Pos = (usize, usize);

/// How much one click picks: a point (a drag from here goes character by character), a word, or the whole message.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Grain { Char, Word, Message }

fn class(c: char) -> u8 { if c.is_alphanumeric() || c == '_' { 0 } else if c.is_whitespace() { 1 } else { 2 } }

/// The word (or the run of spaces, or of punctuation) around a byte offset: what a double-click picks.
pub fn word_at(text: &str, off: usize) -> Range<usize> {
    let off = off.min(text.len());
    if text.is_empty() { return 0..0; }
    // the character at the offset, or the one before it at the very end
    let at = if off < text.len() { off } else { text[..off].char_indices().next_back().map(|(i, _)| i).unwrap_or(0) };
    let at = (0..=at).rev().find(|i| text.is_char_boundary(*i)).unwrap_or(0);
    let k = class(text[at..].chars().next().unwrap());
    let start = text[..at].char_indices().rev().take_while(|(_, c)| class(*c) == k).last().map(|(i, _)| i).unwrap_or(at);
    let end = text[at..].char_indices().find(|(_, c)| class(*c) != k).map(|(i, _)| at + i).unwrap_or(text.len());
    start..end
}

/// The extent one click picks around an offset.
pub fn extent(text: &str, off: usize, g: Grain) -> Range<usize> {
    match g { Grain::Char => off..off, Grain::Word => word_at(text, off), Grain::Message => 0..text.len() }
}

/// The selection a drag makes: `anchor` is the extent the first click picked, `head` where the pointer is now (`head_ext` its extent
/// in the same grain). Whichever side the pointer is on, the anchor's extent stays inside. Returns (start, end) in reading order.
pub fn span(anchor: (Pos, Pos), head: Pos, head_ext: (Pos, Pos)) -> (Pos, Pos) {
    if head < anchor.0 { (head_ext.0, anchor.1) } else if head > anchor.1 { (anchor.0, head_ext.1) } else { anchor }
}

/// The part of row `ix` (its text is `len` bytes) a span covers, or None when it covers nothing of it.
pub fn range_in(s: (Pos, Pos), ix: usize, len: usize) -> Option<Range<usize>> {
    let ((a, ao), (b, bo)) = s;
    if ix < a || ix > b { return None; }
    let r = (if ix == a { ao.min(len) } else { 0 })..(if ix == b { bo.min(len) } else { len });
    (r.start < r.end).then_some(r)
}

/// What Ctrl+C puts on the clipboard. One message: just the words picked. Several: a line each, "Name: words".
/// `rows` are (name, text) for every row, `None` for a row with nothing selectable.
pub fn copy_text(s: (Pos, Pos), rows: &[Option<(&str, &str)>]) -> String {
    let parts: Vec<(&str, &str)> = (s.0 .0..=s.1 .0).filter_map(|ix| {
        let (name, text) = (*rows.get(ix)?)?;
        let r = range_in(s, ix, text.len())?;
        Some((name, text.get(r)?))
    }).collect();
    match parts.as_slice() { [] => String::new(), [(_, t)] => t.to_string(), many => many.iter().map(|(n, t)| format!("{n}: {t}")).collect::<Vec<_>>().join("\n") }
}

/// A row's text on screen: its top and bottom (in the window).
#[derive(Clone, Copy, Debug)]
pub struct Band { pub ix: usize, pub top: f32, pub bottom: f32 }

/// The row a pointer at height `y` belongs to: the one it is over, else the nearest (a drag past the end picks the first or last).
pub fn nearest(bands: &[Band], y: f32) -> Option<usize> {
    bands.iter().min_by(|a, b| gap(a, y).total_cmp(&gap(b, y)).then(a.ix.cmp(&b.ix))).map(|b| b.ix)
}
fn gap(b: &Band, y: f32) -> f32 { if y < b.top { b.top - y } else if y > b.bottom { y - b.bottom } else { 0. } }

/// Rows the reading zone must keep whole: the one under the pointer and any that the selection touches (the web: `.msg:hover`, and
/// `sel.containsNode`).
pub fn keeps_whole(ix: usize, hover: Option<usize>, s: Option<(Pos, Pos)>) -> bool {
    hover == Some(ix) || s.is_some_and(|((a, ao), (b, bo))| ix >= a && ix <= b && !(a == b && ao == bo))
}

/// One row's painted text.
#[derive(Clone)]
pub struct RowText { pub layout: TextLayout, pub bounds: Bounds<Pixels> }
pub type Texts = Rc<RefCell<HashMap<String, RowText>>>;

/// The first click's extent, by message id.
#[derive(Clone)]
struct Anchor { id: String, a: usize, b: usize }

/// The room's selection and what goes with it.
pub struct Sel {
    pub focus: FocusHandle,
    pub texts: Texts,
    /// The list area as painted last (the pointer is only watched inside it, and the menu is placed against it).
    pub area: Rc<Cell<Bounds<Pixels>>>,
    anchor: Option<Anchor>,
    head: Option<(String, usize)>,
    grain: Grain,
    pub dragging: bool,
    pub hover: Option<String>,
    /// The right-click menu: where it is (in the window) and the message it was opened on.
    pub menu: Option<(Point<Pixels>, String)>,
    /// While dragging past the top or bottom of the list: which way (-1, 1, 0) and the pointer.
    pub edge: f32,
    pub pointer: Point<Pixels>,
    /// The resolved selection of the frame being drawn.
    pub now: Option<(Pos, Pos)>,
    /// A phone: where a finger went down on the list (a tap that stays near it opens the message's menu; a drag is the list scrolling).
    pub tap: Option<Point<Pixels>>,
}
impl Sel {
    pub fn new(cx: &mut App) -> Sel {
        Sel { focus: cx.focus_handle(), texts: Default::default(), area: Default::default(), anchor: None, head: None, grain: Grain::Char, dragging: false, hover: None, menu: None, edge: 0., pointer: point(px(0.), px(0.)), now: None, tap: None }
    }
    pub fn clear(&mut self) { self.anchor = None; self.head = None; self.dragging = false; self.edge = 0.; self.now = None; }
    pub fn is_empty(&self) -> bool { self.now.is_none() }
}

pub const MENU_W: f32 = 284.;
/// The row of quick reactions on top of the menu of a message.
pub const MENU_EMOJI: f32 = 44.;
pub fn menu_row() -> f32 { crate::phone::pick(30., crate::phone::TAP) }

impl Mega {
    fn row_of(&self, id: &str) -> Option<usize> { self.win.msgs.iter().position(|m| m.id == id) }
    fn text_of(&self, ix: usize) -> &str { self.win.msgs.get(ix).map(|m| m.shown()).unwrap_or("") }

    /// Work out the selection (row indexes may have moved since it was made): called after every change and when the room is drawn.
    pub fn sel_resolve(&mut self) {
        self.sel.now = match (&self.sel.anchor, &self.sel.head) {
            (Some(a), Some((hid, ho))) => match (self.row_of(&a.id), self.row_of(hid)) {
                (Some(ai), Some(hi)) => {
                    let e = extent(self.text_of(hi), *ho, self.sel.grain);
                    let sp = span(((ai, a.a), (ai, a.b)), (hi, *ho), ((hi, e.start), (hi, e.end)));
                    // a point and nothing else is no selection
                    (sp.0 != sp.1).then_some(sp)
                }
                _ => None,
            },
            _ => None,
        };
    }

    /// Where on screen the pointer is in the text: the row and offset, `strict` only when it is on a row's words (a click starts there).
    fn sel_hit(&self, p: Point<Pixels>, strict: bool) -> Option<(String, usize)> {
        let texts = self.sel.texts.borrow();
        let mut bands: Vec<(Band, &String)> = texts.iter().filter_map(|(id, t)| Some((Band { ix: self.row_of(id)?, top: f32::from(t.bounds.top()), bottom: f32::from(t.bounds.bottom()) }, id))).collect();
        bands.sort_by_key(|(b, _)| b.ix);
        let only: Vec<Band> = bands.iter().map(|(b, _)| *b).collect();
        let ix = nearest(&only, f32::from(p.y))?;
        let id = bands.iter().find(|(b, _)| b.ix == ix)?.1.clone();
        let t = texts.get(&id)?;
        if strict {
            let pad = px(6.);
            let b = Bounds::new(t.bounds.origin - point(pad, pad), t.bounds.size + size(pad * 2., pad * 2.));
            if !b.contains(&p) { return None; }
        }
        let off = t.layout.index_for_position(p).unwrap_or_else(|e| e).min(self.text_of(ix).len());
        Some((id, off))
    }

    /// The row under the pointer (for the reading zone and the right-click menu): by the rows' boxes in the list.
    fn sel_row_at(&self, p: Point<Pixels>) -> Option<usize> {
        let vp = self.list.viewport_bounds();
        if !vp.contains(&p) { return None; }
        (0..self.win.msgs.len()).find(|ix| self.list.bounds_for_item(*ix).is_some_and(|b| b.contains(&p)))
    }

    pub fn sel_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let area = self.sel.area.get();
        let had_menu = self.sel.menu.is_some();
        // the menu: its own items act; anything else closes it
        if let Some((at, id)) = self.sel.menu.clone() {
            let (emoji, items) = self.menu_layout(&id);
            let h = if emoji { MENU_EMOJI } else { 0. } + menu_row() * items.len() as f32 + 8.;
            let at = self.menu_origin(at, h);
            let m = Bounds::new(at, size(px(MENU_W), px(h)));
            if m.contains(&ev.position) {
                if ev.button == MouseButton::Left {
                    let dy = f32::from(ev.position.y - at.y) - 4.;
                    if emoji && dy < MENU_EMOJI {
                        let k = ((f32::from(ev.position.x - at.x) - 8.) / ((MENU_W - 16.) / 8.)).floor().max(0.) as usize;
                        if let Some(e) = crate::msgact::QUICK.get(k) { self.msg_react(&id, e, window, cx); }
                    } else {
                        let k = ((dy - if emoji { MENU_EMOJI } else { 0. }) / menu_row()).floor().max(0.) as usize;
                        match items.get(k).copied() {
                            Some("Copy") => self.sel_copy(cx), Some("Copy message") => self.copy_message(&id, cx), Some("Select all") => self.sel_all(window, cx),
                            Some("Reply") => self.msg_reply(&id, window, cx), Some("Edit") => self.msg_edit(&id, window, cx), Some("Delete") => self.msg_delete(&id, window, cx),
                            _ => {}
                        }
                    }
                    self.sel.menu = None; cx.notify();
                }
                return;
            }
            self.sel.menu = None; cx.notify();
        }
        if !area.contains(&ev.position) || ev.position.x > area.right() - px(14.) {
            if !self.sel.is_empty() { self.sel.clear(); cx.notify(); }
            return;
        }
        match ev.button {
            // a finger scrolls, it does not select: a tap (sel_up) opens the menu, which has Copy message and Select all
            MouseButton::Left if crate::phone::on() => { self.sel.tap = (!had_menu).then_some(ev.position); }
            MouseButton::Left => {
                let Some((id, off)) = self.sel_hit(ev.position, true) else {
                    if !self.sel.is_empty() { cx.notify(); }
                    self.sel.clear(); return;
                };
                let grain = match ev.click_count { 0 | 1 => Grain::Char, 2 => Grain::Word, _ => Grain::Message };
                let ix = self.row_of(&id).unwrap_or(0);
                let e = extent(self.text_of(ix), off, grain);
                self.sel.anchor = Some(Anchor { id: id.clone(), a: e.start, b: e.end });
                self.sel.head = Some((id, off));
                self.sel.grain = grain;
                self.sel.dragging = true;
                self.sel.pointer = ev.position;
                window.focus(&self.sel.focus, cx);
                self.sel_resolve(); cx.notify();
            }
            MouseButton::Right => {
                let Some((id, off)) = self.sel_hit(ev.position, true) else {
                    // not on words (a picture, a file, the gap beside them): the menu of the message under the pointer
                    if let Some(m) = self.sel_row_at(ev.position).and_then(|ix| self.win.msgs.get(ix)).filter(|m| !m.deleted && m.kind != "member" && m.kind != "notice") {
                        let id = m.id.clone(); self.sel.clear(); self.sel.menu = Some((ev.position, id)); cx.notify();
                    }
                    return;
                };
                let ix = self.row_of(&id).unwrap_or(0);
                // a right-click outside the selection drops it; the menu then offers the message
                let inside = self.sel.now.is_some_and(|s| range_in(s, ix, self.text_of(ix).len()).is_some_and(|r| r.contains(&off.min(r.end.saturating_sub(1)))));
                if !inside { self.sel.clear(); }
                let at = ev.position;
                self.sel.menu = Some((at, id));
                cx.notify();
            }
            _ => {}
        }
    }

    pub fn sel_move(&mut self, ev: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let area = self.sel.area.get();
        if let Some(t) = self.sel.tap { if crate::phone::far(t, ev.position) { self.sel.tap = None; } }
        if self.sel.dragging {
            if ev.pressed_button != Some(MouseButton::Left) { self.sel.dragging = false; self.sel.edge = 0.; return; }
            self.sel.pointer = ev.position;
            self.sel.edge = if ev.position.y < area.top() { -1. } else if ev.position.y > area.bottom() { 1. } else { 0. };
            if let Some((id, off)) = self.sel_hit(ev.position, false) { self.sel.head = Some((id, off)); self.sel_resolve(); }
            cx.notify();
            return;
        }
        // which row the pointer is over: that row stays whole in the reading zone
        let over = if area.contains(&ev.position) { self.sel_row_at(ev.position).and_then(|ix| self.win.msgs.get(ix)).map(|m| m.id.clone()) } else { None };
        if over != self.sel.hover { self.sel.hover = over; cx.notify(); }
    }

    pub fn sel_up(&mut self, ev: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = self.sel.tap.take() {
            if !crate::phone::far(t, ev.position) && self.sel.menu.is_none() {
                // a tap on words (not on a picture, a pill or a button: those act themselves; a long press opens the menu anywhere on the row)
                if let Some((id, _)) = self.sel_hit(ev.position, true) { self.sel.clear(); self.sel.menu = Some((ev.position, id)); cx.notify(); }
            }
            return;
        }
        if !self.sel.dragging { return; }
        self.sel.dragging = false; self.sel.edge = 0.;
        // a click that selected nothing gives the keyboard back to the composer
        if self.sel.is_empty() { self.sel.clear(); self.composer.update(cx, |c, cx| c.focus(window, cx)); }
        cx.notify();
    }

    /// While the pointer is held past the list's top or bottom the list keeps scrolling and the selection grows.
    pub fn sel_autoscroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.sel.dragging || self.sel.edge == 0. { return; }
        self.list.scroll_by(px(14. * self.sel.edge));
        if let Some((id, off)) = self.sel_hit(self.sel.pointer, false) { self.sel.head = Some((id, off)); self.sel_resolve(); }
        window.request_animation_frame(); cx.notify();
    }

    pub fn sel_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let ctl = k.modifiers.control || k.modifiers.platform;
        match (k.key.as_str(), ctl) {
            ("c", true) => { self.sel_copy(cx); cx.stop_propagation(); }
            ("a", true) => { self.sel_all(window, cx); cx.stop_propagation(); }
            ("escape", _) => { self.sel.clear(); self.sel.menu = None; self.composer.update(cx, |c, cx| c.focus(window, cx)); cx.notify(); }
            (_, false) => {
                // typing with words picked goes to the composer, as it would have without the selection
                if let Some(ch) = k.key_char.as_ref().filter(|c| !c.is_empty() && !k.modifiers.alt) {
                    let ch = ch.clone();
                    self.sel.clear();
                    self.composer.update(cx, |c, cx| { c.focus(window, cx); let v = format!("{}{}", c.value(), ch); c.set_value(v, window, cx); });
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    /// Everything in the open room.
    pub fn sel_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rows: Vec<usize> = (0..self.win.msgs.len()).filter(|ix| self.selectable(*ix)).collect();
        let (Some(first), Some(last)) = (rows.first().copied(), rows.last().copied()) else { return };
        self.sel.anchor = Some(Anchor { id: self.win.msgs[first].id.clone(), a: 0, b: 0 });
        self.sel.head = Some((self.win.msgs[last].id.clone(), self.win.msgs[last].shown().len()));
        self.sel.grain = Grain::Char; self.sel.dragging = false;
        window.focus(&self.sel.focus, cx);
        self.sel_resolve(); cx.notify();
    }

    fn selectable(&self, ix: usize) -> bool { self.win.msgs.get(ix).is_some_and(|m| m.kind == "text" && !m.deleted && !m.text.is_empty() && m.file.is_none()) }

    /// What is selected, as it is copied.
    pub fn sel_string(&self) -> String {
        let Some(s) = self.sel.now else { return String::new() };
        let rows: Vec<Option<(&str, &str)>> = (0..self.win.msgs.len()).map(|ix| self.selectable(ix).then(|| (self.win.msgs[ix].name.as_str(), self.win.msgs[ix].shown()))).collect();
        copy_text(s, &rows)
    }

    pub fn sel_copy(&mut self, cx: &mut Context<Self>) {
        let t = self.sel_string();
        if !t.is_empty() { cx.write_to_clipboard(ClipboardItem::new_string(t)); }
    }

    pub fn copy_message(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(m) = self.win.msgs.iter().find(|m| m.id == id) { if !m.text.is_empty() { cx.write_to_clipboard(ClipboardItem::new_string(m.shown().to_string())); } }
    }

    /// Where the menu is drawn: at the click, moved in when it would run out of the list's area.
    pub fn menu_origin(&self, at: Point<Pixels>, h: f32) -> Point<Pixels> {
        let area = self.sel.area.get();
        let x = f32::from(at.x).min(f32::from(area.right()) - MENU_W - 8.).max(f32::from(area.left()) + 4.);
        let y = f32::from(at.y).min(f32::from(area.bottom()) - h - 4.).max(f32::from(area.top()) + 4.);
        point(px(x), px(y))
    }

    /// The menu of a message: the quick reactions on top, then what may be done with it (mega-touch.js `menuFor`, and Copy). "Copy" only with a selection.
    pub fn menu_layout(&self, id: &str) -> (bool, Vec<&'static str>) {
        let mut v = vec![];
        let Some(m) = self.win.msgs.iter().find(|m| m.id == id) else { return (false, v) };
        v.push("Reply");
        if !self.sel.is_empty() { v.push("Copy"); }
        if !m.text.is_empty() { v.push("Copy message"); }
        if m.mine && m.kind == "text" { v.push("Edit"); }
        if m.mine || self.is_admin_here() { v.push("Delete"); }
        if self.win.msgs.iter().any(|m| m.kind == "text") { v.push("Select all"); }
        (true, v)
    }

    /// For the test port: the selection as copied, and the pixel where a character of a message is drawn.
    pub fn sel_dump(&self) -> serde_json::Value {
        let texts = self.sel.texts.borrow();
        let rows: Vec<_> = texts.iter().filter_map(|(id, t)| { let ix = self.row_of(id)?; Some(serde_json::json!({"ix": ix, "id": id, "x": f32::from(t.bounds.left()), "y": f32::from(t.bounds.top()), "w": f32::from(t.bounds.size.width), "h": f32::from(t.bounds.size.height), "text": self.text_of(ix)})) }).collect();
        let menu_at = self.sel.menu.clone().map(|(at, id)| { let (e, items) = self.menu_layout(&id); let h = if e { MENU_EMOJI } else { 0. } + menu_row() * items.len() as f32 + 8.; let o = self.menu_origin(at, h); serde_json::json!({"x": f32::from(o.x), "y": f32::from(o.y), "items": items, "emoji": e}) });
        serde_json::json!({"selected": self.sel_string(), "rows": rows, "menu": self.sel.menu.is_some(), "menu_at": menu_at})
    }
}

#[cfg(test)]
mod tests {
    use super::{extent as _e, keeps_whole, nearest, range_in, span, copy_text, word_at, Band};

    #[test]
    fn a_double_click_takes_the_word_or_the_run_of_spaces_or_punctuation() {
        let t = "hello, big  world";
        assert_eq!(word_at(t, 1), 0..5);
        assert_eq!(word_at(t, 5), 5..6); // the comma
        assert_eq!(word_at(t, 6), 6..7);
        assert_eq!(word_at(t, 11), 10..12); // two spaces
        assert_eq!(word_at(t, 17), 12..17); // the very end takes the last word
        assert_eq!(word_at("", 0), 0..0);
        assert_eq!(word_at("héllo wörld", 2), 0..6); // inside a two-byte letter
        assert_eq!(word_at("a_b c", 1), 0..3);
    }
    #[test]
    fn a_drag_keeps_the_first_click_inside_and_grows_by_the_grain() {
        // double-click on "big" (4..7 of row 2) then drag back to row 1 offset 3 (word grain: the word there is 0..5)
        let anchor = ((2, 4), (2, 7));
        assert_eq!(span(anchor, (1, 3), ((1, 0), (1, 5))), ((1, 0), (2, 7)));
        assert_eq!(span(anchor, (2, 20), ((2, 20), (2, 25))), ((2, 4), (2, 25)));
        assert_eq!(span(anchor, (2, 5), ((2, 4), (2, 7))), anchor); // inside it
        // a plain drag
        assert_eq!(span(((3, 2), (3, 2)), (3, 9), ((3, 9), (3, 9))), ((3, 2), (3, 9)));
        assert_eq!(span(((3, 2), (3, 2)), (1, 4), ((1, 4), (1, 4))), ((1, 4), (3, 2)));
    }
    #[test]
    fn each_row_gets_its_part_of_a_selection_across_rows() {
        let s = ((1, 3), (3, 2));
        assert_eq!(range_in(s, 0, 10), None);
        assert_eq!(range_in(s, 1, 10), Some(3..10));
        assert_eq!(range_in(s, 2, 8), Some(0..8));
        assert_eq!(range_in(s, 3, 10), Some(0..2));
        assert_eq!(range_in(s, 4, 10), None);
        assert_eq!(range_in(((1, 3), (1, 3)), 1, 10), None);
        assert_eq!(range_in(((1, 3), (3, 99)), 3, 10), Some(0..10)); // never past the text
    }
    #[test]
    fn one_message_copies_plain_and_several_copy_as_name_lines() {
        let rows = [Some(("Ana", "good morning all")), None, Some(("Ben", "morning!")), Some(("Ana", "coffee?"))];
        assert_eq!(copy_text(((0, 5), (0, 12)), &rows), "morning");
        assert_eq!(copy_text(((0, 5), (3, 3)), &rows), "Ana: morning all\nBen: morning!\nAna: cof");
        assert_eq!(copy_text(((0, 0), (0, 0)), &rows), "");
        // a row with nothing selectable in between is skipped, one at the start does not stop the rest
        assert_eq!(copy_text(((1, 0), (2, 4)), &rows), "morn");
    }
    #[test]
    fn a_pointer_belongs_to_the_row_it_is_over_or_the_nearest() {
        // the reading zone draws the upper rows smaller: their text boxes are shorter and sit lower inside their (taller) row boxes
        let bands = [Band { ix: 4, top: 20., bottom: 34. }, Band { ix: 5, top: 80., bottom: 110. }, Band { ix: 6, top: 130., bottom: 160. }];
        assert_eq!(nearest(&bands, 25.), Some(4));
        assert_eq!(nearest(&bands, 100.), Some(5));
        assert_eq!(nearest(&bands, 0.), Some(4)); // above everything
        assert_eq!(nearest(&bands, 900.), Some(6)); // below everything
        assert_eq!(nearest(&bands, 120.), Some(5)); // in the gap, closer to row 5 (10 away) than 6 (10 away): the first
        assert_eq!(nearest(&bands, 124.), Some(6));
        assert_eq!(nearest(&[], 5.), None);
    }
    #[test]
    fn the_row_under_the_pointer_and_the_selected_rows_stay_whole() {
        let s = Some(((2, 1), (4, 3)));
        assert!(keeps_whole(2, None, s) && keeps_whole(3, None, s) && keeps_whole(4, None, s));
        assert!(!keeps_whole(1, None, s) && !keeps_whole(5, None, s));
        assert!(keeps_whole(7, Some(7), s) && !keeps_whole(7, Some(8), None));
        assert!(!keeps_whole(2, None, Some(((2, 1), (2, 1))))); // a caret selects nothing
    }
}
