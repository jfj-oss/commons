//! The scroll-focus column's rules (app/ui/mega.js `List`, mega.css `.mlist .mslot .mcard`), without any drawing.
//!
//! The communities of the home screen and the rooms of a community are one vertical column of squircle cards. The card nearest
//! the middle of the window is the focused one: full size, with its field moving and its words big. The others shrink with
//! their distance from the middle (`scale(1 - .26 k)`, k = distance in card pitches, at most 1.5) and fade (`1 - .72 a`).
//! After the scroll has been still for 150 ms the focused card "settles" (its title turns sharp, its preview appears, it grows
//! from .9 to 1). Resting only enlarges: opening a card needs a click or Enter. All of it is in this file so it can be tested.
//!
//! Positions are `offsets`: the scroll offset at which card i is centred. In the page that is the sum of the heights of
//! everything before it (a slot is 70% of the column's height, a section label 16%), because the list pads 15% above and below.

/// A slot (one card and the gap to the next) is this share of the column's height (`.mslot { height: 70cqh }`).
pub const SLOT: f32 = 0.70;
/// The card itself is this share (`.mcard { height: 80cqh }`) and at most this share of the height wide (`max-width: min(1800px, 72cqh)`).
pub const CARD_H: f32 = 0.80;
pub const CARD_W: f32 = 0.72;
/// A section label such as "voice" (`.msect { height: 16cqh }`).
pub const SECT: f32 = 0.16;
/// Side gutter of the column (`.mlist { padding-inline: 16px }`).
pub const GUTTER: f32 = 16.;
/// Corner of a full-size card (`--m-sq: 76px`).
pub const SQ: f32 = 76.;
/// The scroll stays this long without input before the card settles (mega.js `t = setTimeout(settle, 150)`).
pub const SETTLE_AFTER: f64 = 0.15;
/// A card is drawn while it is within this many slots of the middle (`visibility: hidden` beyond 1.7), and its field moves within 1.35.
pub const SHOWN: f32 = 1.7;
pub const LIVE: f32 = 1.35;
/// What an unsettled card is scaled by on top of its distance (`.msq { transform: scale(.9) }`, none once `.settled`).
pub const UNSETTLED: f32 = 0.9;

/// The height of a slot in a column this high.
pub fn slot(h: f32) -> f32 { SLOT * h }

/// Size of a full-size card in a column `w` wide and `h` high.
pub fn card_box(w: f32, h: f32) -> (f32, f32) { ((CARD_W * h).min(1800.).min((w - 2. * GUTTER).max(40.)), CARD_H * h) }

/// The scroll offset that centres each card; `sections[i]` says a section label comes before card i.
pub fn offsets(h: f32, sections: &[bool]) -> Vec<f32> {
    let mut at = 0.;
    sections.iter().map(|&s| { if s { at += SECT * h; } let o = at; at += SLOT * h; o }).collect()
}

/// Distance of card `at` from the middle, in slots.
pub fn dist(offset: f32, at: f32, slot: f32) -> f32 { if slot <= 0. { 0. } else { (at - offset).abs() / slot } }
/// How much a card at distance `a` is scaled (the card's own `transform`).
pub fn scale(a: f32) -> f32 { 1. - 0.26 * a.min(1.5) }
pub fn opacity(a: f32) -> f32 { (1. - 0.72 * a).clamp(0., 1.) }
pub fn shown(a: f32) -> bool { a <= SHOWN }
pub fn live(a: f32) -> bool { a < LIVE }

/// The card nearest the middle (the first one on a tie).
pub fn nearest(offsets: &[f32], offset: f32) -> usize {
    let mut best = 0;
    for (i, o) in offsets.iter().enumerate() { if (o - offset).abs() < (offsets[best] - offset).abs() - 1e-3 { best = i; } }
    best
}

/// Where a scroll that has just stopped comes to rest: the nearest card to where it would have coasted on. `vel` is px/s.
pub fn snap_to(offsets: &[f32], offset: f32, vel: f32) -> usize { nearest(offsets, offset + vel * 0.12) }

/// A title's font size at a card width `cw` (`.mti`: clamp(40px, 13cqw, 138px) x 1.25, rounded down to the 12 px grid of the pixel font).
pub fn title_px(cw: f32) -> f32 { snap_down((0.13 * cw).clamp(40., 138.) * 1.25, 12.) }
/// The unread number's size (`.mnu`: clamp(64px, 25cqw, 210px) on the same grid).
pub fn number_px(cw: f32) -> f32 { snap_down((0.25 * cw).clamp(64., 210.), 12.) }
/// The padding of a card (`.msq`: clamp(22px, 3.2cqw, 48px)), the line above the title (clamp(14px, 1.3cqw, 18px)), the preview (clamp(17px, 1.9cqw, 26px)).
pub fn pad(cw: f32) -> f32 { (0.032 * cw).clamp(22., 48.) }
pub fn top_px(cw: f32) -> f32 { (0.013 * cw).clamp(14., 18.) }
pub fn preview_px(cw: f32) -> f32 { (0.019 * cw).clamp(17., 26.) }
/// CSS `round(down, v, step)`, at least one step.
pub fn snap_down(v: f32, step: f32) -> f32 { ((v / step).floor() * step).max(step) }
/// A font size on the half-pixel grid, so a card that grows by a hair does not ask the glyph atlas for a new size every frame.
pub fn q(v: f32) -> f32 { (v * 2.).round() / 2. }

/// `cubic-bezier(.2, 1.2, .3, 1)` is a gentle overshoot; this is the same feel: ease out with a little back.
pub fn spring(t: f32) -> f32 { let t = t.clamp(0., 1.) - 1.; 1. + 2.2 * t * t * t + 1.2 * t * t }
fn ease_out(t: f32) -> f32 { let t = 1. - t.clamp(0., 1.); 1. - t * t * t }

#[derive(Clone, Copy, Debug)]
struct Anim { from: f32, to: f32, t0: f64, dur: f64 }

/// The scroll position, its smooth moves and its settling.
#[derive(Clone, Debug)]
pub struct Scroll {
    pub offset: f32,
    anim: Option<Anim>,
    /// A touchpad or other pixel-exact scroll is under way and has to be snapped when it stops.
    coasting: bool,
    last_input: f64,
    last_tick: f64,
    vel: f32,
    /// Which card is settled and the offset it settled at.
    pub settled: Option<usize>,
    settled_at: f32,
    /// 0 = unsettled .. 1 = fully settled: what the card's size, preview and title cut follow.
    pub grow: f32,
    /// The card the growth belongs to: the one that settled last. A card that stops being it shrinks back at once (`.mslot.settled` is removed).
    pub focus: usize,
    pub cur: usize,
    /// Battery saver: the same layout and the same snapping, but every move is a jump and nothing grows over time.
    pub calm: bool,
}

impl Default for Scroll {
    fn default() -> Self { Scroll { offset: 0., anim: None, coasting: false, last_input: -1e9, last_tick: 0., vel: 0., settled: None, settled_at: 0., grow: 0., focus: 0, cur: 0, calm: false } }
}

impl Scroll {
    /// Start on card `i`, as if it had just been scrolled to (not yet settled).
    pub fn start(i: usize, offsets: &[f32], now: f64) -> Scroll {
        let mut s = Scroll::default();
        s.offset = offsets.get(i).copied().unwrap_or(0.); s.cur = i; s.focus = i; s.last_input = now; s.last_tick = now;
        s
    }
    pub fn moving(&self) -> bool { self.anim.is_some() || self.coasting }
    /// The card a move would start from: where the current glide ends, or the card nearest the middle.
    pub fn target(&self, offsets: &[f32]) -> usize { match self.anim { Some(a) => nearest(offsets, a.to), None => nearest(offsets, self.offset) } }
    fn disturb(&mut self, now: f64) { self.last_input = now; }
    /// Pixel-exact scrolling (touchpad): the column follows the fingers.
    pub fn wheel_px(&mut self, dy: f32, offsets: &[f32], now: f64) {
        let max = offsets.last().copied().unwrap_or(0.);
        let dt = (now - self.last_tick).clamp(0.004, 0.2) as f32;
        self.vel = self.vel * 0.6 + (dy / dt) * 0.4;
        self.anim = None; self.coasting = true;
        self.offset = (self.offset + dy).clamp(0., max);
        self.disturb(now);
    }
    /// One notch of a wheel, or an arrow key: to the next card in that direction.
    pub fn step(&mut self, d: i32, offsets: &[f32], now: f64) {
        if offsets.is_empty() { return; }
        let i = (self.target(offsets) as i64 + d as i64).clamp(0, offsets.len() as i64 - 1) as usize;
        self.go(i, offsets, now, false);
    }
    /// A smooth move to card `i`, or at once.
    pub fn go(&mut self, i: usize, offsets: &[f32], now: f64, at_once: bool) {
        let Some(&to) = offsets.get(i.min(offsets.len().saturating_sub(1))) else { return };
        self.coasting = false; self.vel = 0.; self.disturb(now);
        if at_once || self.calm || (to - self.offset).abs() < 0.5 { self.offset = to; self.anim = None; return; }
        let cards = ((to - self.offset).abs() / (offsets.get(1).copied().unwrap_or(1.).max(1.))).max(0.2);
        self.anim = Some(Anim { from: self.offset, to, t0: now, dur: (0.30 + 0.12 * (cards as f64).min(4.)).min(0.7) });
    }
    /// Advance to `now`. True while another frame is needed: a glide, a coast waiting to be snapped, the settling, or the settled card growing.
    pub fn tick(&mut self, now: f64, offsets: &[f32]) -> bool {
        let dt = (now - self.last_tick).clamp(0., 0.25) as f32; self.last_tick = now;
        if let Some(a) = self.anim {
            let t = ((now - a.t0) / a.dur) as f32;
            self.offset = if t >= 1. { a.to } else { a.from + (a.to - a.from) * ease_out(t) };
            if t >= 1. { self.anim = None; self.last_input = now; }
            else { self.last_input = now; }
        }
        self.cur = nearest(offsets, self.offset).min(offsets.len().saturating_sub(1));
        if self.settled.is_some() && (self.offset - self.settled_at).abs() > 3. { self.settled = None; }
        if self.anim.is_none() && now - self.last_input >= SETTLE_AFTER {
            if self.coasting {
                // the fingers are up: come to rest on a card
                self.coasting = false;
                let i = snap_to(offsets, self.offset, self.vel); self.vel = 0.;
                let Some(&to) = offsets.get(i) else { return false };
                if self.calm { self.offset = to; self.cur = nearest(offsets, to); }
                else if (to - self.offset).abs() > 0.5 { self.anim = Some(Anim { from: self.offset, to, t0: now, dur: 0.3 }); self.last_input = now; }
            } else if self.settled != Some(self.cur) && !offsets.is_empty() {
                self.settled = Some(self.cur); self.settled_at = self.offset;
                if self.focus != self.cur { self.focus = self.cur; self.grow = 0.; }
            }
        }
        // the card grows once settled (a spring over .75 s) and shrinks quickly when it moves again
        let want = if self.settled.is_some() { 1. } else { 0. };
        if self.calm { self.grow = want; } else if want > self.grow { self.grow = (self.grow + dt / 0.6).min(1.); } else if want < self.grow { self.grow = (self.grow - dt / 0.25).max(0.); }
        self.anim.is_some() || self.coasting || self.settled.is_none() || (self.grow - want).abs() > 1e-3
    }
    /// What the focused card is scaled by on top of its distance (.9 unsettled, 1 settled, a little over while it springs).
    pub fn grown(&self) -> f32 { UNSETTLED + (1. - UNSETTLED) * spring(self.grow) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn scale_and_fade_follow_the_distance() {
        assert_eq!(scale(0.), 1.);
        assert!((scale(1.) - 0.74).abs() < 1e-6, "the next card is .74 (the web shows scale(0.74))");
        assert!((scale(2.) - scale(1.5)).abs() < 1e-6 && (scale(1.5) - 0.61).abs() < 1e-6, "k stops at 1.5");
        assert_eq!(opacity(0.), 1.);
        assert!((opacity(1.) - 0.28).abs() < 1e-6);
        assert_eq!(opacity(2.), 0.);
        assert!(shown(1.7) && !shown(1.71));
        assert!(live(1.34) && !live(1.35));
    }
    #[::core::prelude::v1::test]
    fn the_card_is_a_tall_square_in_the_middle_of_the_slot() {
        // 1280x900: 720 high, 648 wide (the web's measured card), the next card's centre 630 away
        let (w, h) = card_box(1280., 900.);
        assert_eq!((w, h), (648., 720.));
        assert_eq!(slot(900.), 630.);
        // a narrow window: the card fills the width less the gutters
        assert_eq!(card_box(400., 900.).0, 368.);
        // a very tall window: never wider than 1800
        assert_eq!(card_box(5000., 4000.).0, 1800.);
    }
    #[::core::prelude::v1::test]
    fn offsets_add_the_section_label_before_voice() {
        let o = offsets(1000., &[false, false, true, false]);
        assert_eq!(o, vec![0., 700., 700. * 2. + 160., 700. * 3. + 160.]);
        assert_eq!(offsets(1000., &[]), Vec::<f32>::new());
    }
    #[::core::prelude::v1::test]
    fn nearest_is_the_card_whose_centre_is_closest_to_the_middle() {
        let o = offsets(1000., &[false, false, false]);
        assert_eq!(nearest(&o, 0.), 0);
        assert_eq!(nearest(&o, 349.), 0);
        assert_eq!(nearest(&o, 351.), 1);
        assert_eq!(nearest(&o, 350.), 0, "a tie goes to the first");
        assert_eq!(nearest(&o, 5000.), 2);
        assert_eq!(nearest(&[], 5.), 0);
    }
    #[::core::prelude::v1::test]
    fn a_stopped_scroll_snaps_toward_where_it_was_going() {
        let o = offsets(1000., &[false, false, false]);
        assert_eq!(snap_to(&o, 300., 0.), 0);
        assert_eq!(snap_to(&o, 300., 1500.), 1, "a quick flick forward reaches the next card");
        assert_eq!(snap_to(&o, 400., -1500.), 0);
    }
    #[::core::prelude::v1::test]
    fn the_pixel_font_sizes_follow_the_web_rules() {
        // card 648 wide: title 13cqw = 84 x 1.25 = 105 -> 96; number 25cqw = 162 -> 156 (the web's measured sizes)
        assert_eq!(title_px(648.), 96.);
        assert_eq!(number_px(648.), 156.);
        assert_eq!(pad(648.), 22.);
        assert_eq!(top_px(648.), 14.);
        assert_eq!(title_px(100.), 48., "never below 40 x 1.25 = 50 on the 12 px grid");
        assert_eq!(title_px(5000.), 168., "138 x 1.25 = 172.5 -> 168");
        assert_eq!(snap_down(5., 12.), 12.);
        assert_eq!(q(13.26), 13.5);
    }
    #[::core::prelude::v1::test]
    fn a_wheel_notch_moves_one_card_and_settles_after_a_pause() {
        let o = offsets(1000., &[false; 5]);
        let mut s = Scroll::start(0, &o, 0.);
        assert!(s.tick(0.01, &o), "starts unsettled and wants frames");
        s.tick(0.2, &o);
        assert_eq!(s.settled, Some(0));
        s.step(1, &o, 1.0);
        s.step(1, &o, 1.05);
        s.tick(1.1, &o);
        assert_eq!(s.settled, None, "moving unsettles");
        // two quick notches go two cards, and the glide ends exactly on the card
        let mut t = 1.1;
        while t < 3. { t += 0.016; s.tick(t, &o); }
        assert!((s.offset - o[2]).abs() < 1e-3, "{} vs {}", s.offset, o[2]);
        assert_eq!(s.settled, Some(2));
        assert!(!s.tick(t + 1., &o), "a settled column asks for no more frames");
    }
    #[::core::prelude::v1::test]
    fn a_touchpad_scroll_follows_the_fingers_then_snaps() {
        let o = offsets(1000., &[false; 4]);
        let mut s = Scroll::start(1, &o, 0.);
        s.tick(0.2, &o);
        let mut t = 1.0;
        for _ in 0..10 { s.wheel_px(-20., &o, t); t += 0.016; s.tick(t, &o); }
        assert!((s.offset - (o[1] - 200.)).abs() < 1e-3, "followed exactly: {}", s.offset);
        assert_eq!(s.settled, None);
        while t < 3. { t += 0.016; s.tick(t, &o); }
        assert!(o.iter().any(|x| (x - s.offset).abs() < 1e-3), "rests on a card: {}", s.offset);
        assert_eq!(s.settled, Some(s.cur));
        // clamped at the ends
        s.wheel_px(-99999., &o, t + 1.); assert_eq!(s.offset, 0.);
        s.wheel_px(99999., &o, t + 1.1); assert_eq!(s.offset, *o.last().unwrap());
    }
    #[::core::prelude::v1::test]
    fn the_settled_card_grows_from_point_nine_to_one() {
        let o = offsets(1000., &[false; 2]);
        let mut s = Scroll::start(0, &o, 0.);
        assert!((s.grown() - 0.9).abs() < 1e-6);
        let mut t = 0.; while t < 2. { t += 0.016; s.tick(t, &o); }
        assert_eq!(s.grow, 1.);
        assert!((s.grown() - 1.).abs() < 1e-5);
        // the spring never goes far past 1
        assert!(spring(0.7) < 1.06 && spring(0.) == 0. && spring(1.) == 1.);
    }
    #[::core::prelude::v1::test]
    fn go_clamps_and_can_jump() {
        let o = offsets(1000., &[false; 3]);
        let mut s = Scroll::start(0, &o, 0.);
        s.go(99, &o, 1., true);
        assert_eq!(s.offset, o[2]);
        s.step(1, &o, 2.);
        assert!(s.anim.is_none(), "already on the last card");
        s.go(0, &o, 3., false);
        assert!(s.anim.is_some());
        assert_eq!(s.target(&o), 0);
    }
}
