//! The first-run tour (mega-tour.js): five short steps, shown once per device after the cards first appear, and again from
//! the options room ("show the tour again", in Settings). A step points at the real element when it is on screen (the places
//! are read from the last frame, `Hits`); otherwise the note stands alone in the middle, as the page does. Never started on
//! its own while a test drives the window (`MEGA_GPUI_UI_TEST`) unless `MEGA_GPUI_TOUR=on`; `MEGA_GPUI_TOUR=off` never.
use crate::app::{Level, Mega, Screen};
use crate::look;
use crate::squircle as sq;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

pub const STEPS: [(&str, &str); 5] = [
    ("tour-card", "these cards are your communities. scroll to move through them, click one to go in."),
    ("tour-card2", "inside, rooms are cards too. a voice room joins with one click, and the call dock opens on the right."),
    ("tour-rail", "in a chat, the dots on the left are your rooms. hover or scroll on them to switch."),
    ("tour-draw", "draw lets everyone doodle on the room's backdrop. you find it in the bar at the top."),
    ("tour-opts", "the options room has your settings, and a link to send friends who want to get mega."),
];

pub struct TourUi { pub open: bool, pub step: usize, pub calm: u32, pub hits: crate::calldock::Hits, pub focus: FocusHandle }
impl TourUi {
    pub fn new(cx: &mut Context<Mega>) -> TourUi { TourUi { open: false, step: 0, calm: 0, hits: Default::default(), focus: cx.focus_handle() } }
}

/// The note's place: right of the thing, else under it, else above it; always inside the window (mega-tour.js `place`).
pub fn place(r: (f32, f32, f32, f32), vw: f32, vh: f32, bh: f32) -> (f32, f32, f32) {
    let w = 320f32.min(vw - 24.);
    let (mut left, mut top) = (r.0 + r.2 + 16., r.1);
    if left + w > vw - 12. {
        left = r.0.max(12.).min(vw - w - 12.); top = r.1 + r.3 + 16.;
        if top + bh > vh - 12. { top = (r.1 - bh - 16.).max(12.); }
    }
    (left, top.max(12.).min(vh - bh - 12.), w)
}

impl Mega {
    pub fn tour_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tour.open { return; }
        self.tour.open = true; self.tour.step = 0;
        window.focus(&self.tour.focus, cx);
        cx.notify();
    }
    pub fn tour_end(&mut self, done: bool, cx: &mut Context<Self>) {
        if !self.tour.open { return; }
        self.tour.open = false;
        if done { self.prefs.tour_seen = true; self.prefs.save(); }
        cx.notify();
    }
    fn tour_next(&mut self, cx: &mut Context<Self>) {
        if self.tour.step + 1 >= STEPS.len() { self.tour_end(true, cx); } else { self.tour.step += 1; cx.notify(); }
    }
    /// Every 0.7 s: the first time, once the cards are on screen and nothing else is in the way, two calm beats in a row.
    pub fn tour_tick(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> bool {
        let off = match std::env::var("MEGA_GPUI_TOUR").as_deref() { Ok("off") => true, Ok("on") => false, _ => self.uitest.is_some() };
        if self.tour.open || self.prefs.tour_seen || off { self.tour.calm = 0; return false; }
        let calm = self.screen == Screen::Main && self.level == Level::Home && !self.nav.opens.is_empty() && window.is_window_active() && self.gk.open == false && !self.call.settings_open;
        self.tour.calm = if calm { self.tour.calm + 1 } else { 0 };
        if self.tour.calm >= 2 { self.tour.calm = 0; return true; }
        false
    }
    pub fn tour_dump(&self) -> serde_json::Value {
        serde_json::json!({"open": self.tour.open, "step": self.tour.step, "seen": self.prefs.tour_seen, "text": STEPS[self.tour.step.min(4)].1, "target": self.tour.hits.get(STEPS[self.tour.step.min(4)].0).map(|r| [r.0, r.1, r.2, r.3]), "hits": self.tour.hits.dump()})
    }

    pub fn tour_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.tour.open { return None; }
        let i = self.tour.step.min(STEPS.len() - 1);
        let last = i == STEPS.len() - 1;
        let target = self.tour.hits.get(STEPS[i].0).filter(|r| r.2 > 4. && r.3 > 4.);
        // the places are read from the last frame; only what is drawn this frame writes them again
        self.tour.hits.clear();
        let vp = crate::titlebar::viewport(window);
        let (vw, vh) = (f32::from(vp.width), f32::from(vp.height));
        let bh = 150.;
        let (left, top, w) = match target { Some(r) => place(r, vw, vh, bh), None => (((vw - 320f32.min(vw - 24.)) / 2.).max(12.), ((vh - bh) / 2.).max(12.), 320f32.min(vw - 24.)) };
        let hits = self.tour.hits.clone();
        let btn = |id: &'static str, label: &'static str, primary: bool| div().id(id).relative().h(px(32.)).px(px(16.)).flex().items_center().cursor_pointer().font_family(look::F_UI).text_size(px(13.)).font_weight(FontWeight::SEMIBOLD)
            .child(sq::back(10., if primary { look::paper() } else { rgb(0x0b0a10).into() })).child(sq::front(10., None, Some((1., look::line())), None)).child(div().relative().when(primary, |d| d.text_color(rgb(0x0c0d12))).child(label));
        let ring = target.map(|r| div().absolute().left(px(r.0 - 6.)).top(px(r.1 - 6.)).w(px(r.2 + 12.)).h(px(r.3 + 12.)).rounded(px(18.)).border_2().border_color(look::paper()));
        let note = v_flex().id("tour-box").absolute().left(px(left)).top(px(top)).w(px(w)).occlude().track_focus(&self.tour.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| match ev.keystroke.key.as_str() { "escape" => { cx.stop_propagation(); this.tour_end(true, cx); } "enter" => { cx.stop_propagation(); this.tour_next(cx); } _ => {} }))
            .child(sq::back(20., rgb(0x14131a))).child(sq::front(20., None, Some((1., look::line())), None))
            .child(v_flex().relative().w_full().gap(px(10.)).p(px(18.))
                .child(div().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child(format!("{} of {}", i + 1, STEPS.len())))
                .child(div().id("tour-text").font_family(look::F_UI).text_size(px(14.)).line_height(px(20.)).child(STEPS[i].1))
                .child(h_flex().gap(px(8.)).justify_end()
                    .when(!last, |d| d.child(btn("tour-skip", "skip", false).on_click(cx.listener(|this, _, _, cx| this.tour_end(true, cx)))))
                    .child(btn("tour-next", if last { "done" } else { "next" }, true).on_click(cx.listener(|this, _, _, cx| this.tour_next(cx))))));
        let _ = hits;
        Some(div().id("tour").absolute().inset_0().children(ring).child(note).into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn the_note_stays_inside_the_window() {
        let (l, t, w) = place((100., 100., 200., 100.), 1000., 700., 150.);
        assert_eq!((l, t, w), (316., 100., 320.), "to the right of the thing");
        let (l, t, _) = place((800., 100., 150., 100.), 1000., 700., 150.);
        assert!(l + 320. <= 988. && t >= 216., "no room on the right: under it ({l},{t})");
        let (_, t, _) = place((800., 600., 150., 90.), 1000., 700., 150.);
        assert!(t + 150. <= 688., "no room under it either: above");
        let (l, _, w) = place((10., 10., 20., 20.), 300., 700., 150.);
        assert_eq!(w, 276.); assert!(l >= 12.);
    }
    #[::core::prelude::v1::test]
    fn five_steps_as_the_page_has_them() {
        assert_eq!(STEPS.len(), 5);
        assert!(STEPS[0].1.starts_with("these cards") && STEPS[4].1.contains("options room"));
    }
}
