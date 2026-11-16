//! The room's backdrop (app/ui/mega.js: bdColours, bdDraw, bdLoop, ripple, bdHeat): the room's ASCII field behind the chat, in the
//! owner's chat pattern, colours and strength, dim, flowing around every message's words (ascii.rs `hole`). New activity sends a ripple
//! out of the newest line and warms the room (a little faster, a little brighter); the heat then cools back to its floor.
//! Nothing moves when nothing happens: at rest the field is one still picture, and frames are asked for only while a ripple runs or the
//! room is cooling. The battery saver draws the faint tone-on-tone emboss the cards use, and never moves.
use crate::ascii::{Field, Ripple};
use crate::look::{self, Duo};
use gpui_kit::*;

/// A quiet room still breathes: this much heat is the least it ever settles to (mega.js `HEAT_FLOOR`).
pub const HEAT_FLOOR: f64 = 0.24;
/// The page's backdrop base and the message text colour it must keep readable (mega.js `BD_BASE`, `BD_TEXT`).
pub const BASE: u32 = 0x050407;
const TEXT: u32 = 0xe9e7ef;

/// The open room's backdrop clock: the pattern's time, the room's heat and the ripples running now.
#[derive(Clone, Debug)]
pub struct Bd { pub t: f64, pub heat: f64, pub rips: Vec<Ripple> }

impl Bd {
    pub fn new(gid: &str) -> Bd { Bd { t: (look::h(gid) % 90) as f64, heat: HEAT_FLOOR, rips: vec![] } }
    /// Does anything still move? (Otherwise no frame is asked for.)
    pub fn live(&self) -> bool { !self.rips.is_empty() || self.heat > HEAT_FLOOR + 1e-6 }
    /// `bdLoop`: the time runs faster the hotter the room is, heat cools with a half life of 18 seconds, rings travel out and fade.
    pub fn step(&mut self, dt: f64) {
        self.heat = HEAT_FLOOR.max(self.heat * 0.5f64.powf(dt / 18.));
        self.t += dt * (0.1 + self.heat * 1.5);
        for q in &mut self.rips { q.r += q.v * dt; q.s *= q.fade.powf(dt); }
        self.rips.retain(|q| q.s > 0.04 && q.r < 400.);
    }
    /// `bdHeat`
    pub fn warm(&mut self, n: f64, dim: u32) { self.heat = (self.heat + n * lv(dim)).min(1.); }
    /// `ripple`: a ring from (fx, fy), as fractions of the room; `ring` is the slower, wider kind a thrown reaction makes.
    pub fn ripple(&mut self, fx: f64, fy: f64, s: f64, ring: bool, dim: u32) {
        if dim == 0 { return; }
        if self.rips.len() > 6 { self.rips.remove(0); }
        self.rips.push(Ripple { x: fx, y: fy, r: 0., s: s * lv(dim), v: if ring { 20. } else { 44. }, band: if ring { 7. } else { 4. }, fade: if ring { 0.3 } else { 0.12 }, frac: true });
    }
}

/// The owner's strength as a factor: 100 percent is 1 (mega.js `lvOf`).
pub fn lv(dim: u32) -> f64 { dim as f64 / 100. }

/// The wash behind the whole chat: the main colour, only as strong as the message text can bear (contrast stays 7 or better) (`bdWash`).
pub fn wash(main: u32, lv: f64) -> u32 {
    if lv <= 0. { return BASE; }
    let mut w = (0.4f64).min(0.24 * lv);
    let mut c = look::mix(BASE, main, w as f32);
    let mut i = 0;
    while w > 0.01 && look::contrast(TEXT, c) < 7. && i < 24 { i += 1; w *= 0.88; c = look::mix(BASE, main, w as f32); }
    c
}

fn lift_over(mut c: u32, bg: u32, r: f32) -> u32 { let mut i = 0; while look::contrast(c, bg) < r && i < 16 { i += 1; c = look::mix(c, 0xffffff, 0.12); } c }

/// The glyph colours (`bdColours`): soft, mid and bright, and the brighter three under a ripple; and the wash they lie on.
pub fn tints(duo: Duo, dim: u32) -> ([Hsla; 3], [Hsla; 3], u32) {
    let l = lv(dim);
    let k = l.min(1.) as f32;
    let bg = wash(duo.main, l);
    let hi = lift_over(duo.hi, bg, 3.2);
    let c = |x: u32| -> Hsla { rgb(x).into() };
    let row = |f: u32, h: u32| [c(look::mix(f, h, 0.3)), c(look::mix(f, h, 0.6)), c(look::mix(f, h, 1.))];
    (row(look::mix(bg, hi, 0.38 * k), look::mix(bg, hi, 0.75 * k)), row(look::mix(bg, hi, 0.62 * k), hi), bg)
}

/// The room's field for this frame. `still`: the battery saver's frozen emboss.
pub fn field(gid: &str, fx: usize, duo: Duo, dim: u32, bd: &Bd, still: bool, holes: &crate::ascii::Holes, avoid: Option<(crate::wall::Grid, std::rc::Rc<Vec<(u16, u8, u8)>>)>, narrow: bool) -> Field {
    let l = lv(dim);
    let mut f = Field::card(fx, look::h(gid), duo, if still { 11.7 } else { bd.t }, narrow);
    f.live_holes = Some(holes.clone());
    f.avoid = avoid;
    f.hi_t = 0.86;
    if still { f.emboss = true; f.gate = 0.1; return f; }
    let (t, lit, _) = tints(duo, dim);
    f.tints = Some(t); f.lit = Some(lit);
    f.gate = (0.46 - 0.26 * bd.heat) / l.max(1.);
    f.ripples = bd.rips.clone();
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    fn duo() -> Duo { Duo { main: 0x6a35ff, hi: 0xffb3f0 } }
    #[::core::prelude::v1::test]
    fn the_wash_keeps_the_words_readable_and_follows_the_strength() {
        assert_eq!(wash(0xff5a1f, 0.), BASE, "strength 0: the plain base");
        for m in [0x6a35ff, 0xffd23f, 0xe8e4d8, 0xff5a1f] { assert!(look::contrast(TEXT, wash(m, 1.6)) >= 6.9, "{m:06x}"); }
        assert_ne!(wash(0x6a35ff, 0.4), wash(0x6a35ff, 1.));
    }
    #[::core::prelude::v1::test]
    fn glyph_tints_run_from_the_wash_to_the_highlight() {
        let (t, lit, bg) = tints(duo(), 100);
        assert_eq!(bg, wash(duo().main, 1.));
        assert_ne!(t[0], t[2]);
        assert_ne!(t[2], lit[0], "under a ripple the glyphs are brighter");
    }
    #[::core::prelude::v1::test]
    fn a_quiet_room_asks_for_no_frames() {
        let mut b = Bd::new("g");
        assert!(!b.live());
        let t0 = b.t;
        b.ripple(0.3, 0.8, 0.72, false, 100); b.warm(0.24, 100);
        assert!(b.live());
        let mut n = 0;
        while b.live() && n < 4000 { b.step(0.05); n += 1; }
        assert!(!b.live(), "it settles");
        assert!(b.t > t0, "and the pattern moved meanwhile");
        let t1 = b.t; b.step(0.05);
        assert!(b.t > t1, "stepping always advances time, but nothing steps at rest (app.rs animate)");
    }
    #[::core::prelude::v1::test]
    fn strength_zero_makes_no_ripples_and_a_hotter_room_gates_lower() {
        let mut b = Bd::new("g");
        b.ripple(0.5, 0.5, 1., false, 0);
        assert!(b.rips.is_empty());
        let holes: crate::ascii::Holes = Default::default();
        let cold = field("g", 0, duo(), 100, &b, false, &holes, None, false).gate;
        b.warm(0.5, 100);
        assert!(field("g", 0, duo(), 100, &b, false, &holes, None, false).gate < cold);
        assert!(field("g", 0, duo(), 160, &Bd::new("g"), false, &holes, None, false).gate < cold, "stronger shows more");
    }
}
