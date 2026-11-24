//! The reading zone (app/ui/mega.js `focusFrame`, mega.css `#stream .msg`): while you read back, the rows in the middle of the
//! room are whole, and the further a row is from that band the smaller and fainter it gets. This file is the maths only
//! (no GPUI) so it can be tested, plus how many of the header's chips fit (mega-design.css: the bar never wraps).

/// The band is the middle 40 percent of the room: from 30 to 70 percent of its height (`zt = top + hh * .3`, `zb = top + hh * .7`).
pub const ZONE_TOP: f32 = 0.3;
pub const ZONE_BOTTOM: f32 = 0.7;
/// A row this far (a share of the room's height) outside the band is at the farthest: `reach = hh * .3`.
pub const REACH: f32 = 0.3;
/// The farthest rows shrink by this much (`scale(1 - .2 * d)`) and fade by this much (`opacity: 1 - .55 * d`).
pub const SHRINK: f32 = 0.2;
pub const FADE: f32 = 0.55;
/// Within this many pixels of the end the room is "at the end": no effect (`atEnd`: less than 80 px left to scroll).
pub const AT_END: f32 = 80.;
/// How finely the distance is stepped: the rows are re-laid out in their size, so a step of 1/24 keeps that to a few distinct sizes.
pub const STEPS: f32 = 24.;

/// How far outside the band a row is, 0 (in it) to 1 (the farthest): the row spans `top..bottom` in the room's own coordinates
/// (0 is the room's top edge), `height` is the room's height.
pub fn distance(top: f32, bottom: f32, height: f32) -> f32 {
    if height <= 0. { return 0.; }
    let (zt, zb, reach) = (height * ZONE_TOP, height * ZONE_BOTTOM, height * REACH);
    ((zt - bottom).max(top - zb).max(0.) / reach).min(1.)
}
pub fn scale(d: f32) -> f32 { 1. - SHRINK * d }
pub fn opacity(d: f32) -> f32 { 1. - FADE * d }
/// The distance rounded to a step.
pub fn quantise(d: f32) -> f32 { (d * STEPS).round() / STEPS }
/// Is the room at its end: `remaining` is how many pixels there are left to scroll down.
pub fn at_end(remaining: f32) -> bool { remaining < AT_END }

/// The effect for a row: (scale, opacity) or None when it is whole. `remaining` is the room's pixels left below the view.
pub fn effect(top: f32, bottom: f32, height: f32, remaining: f32) -> Option<(f32, f32)> {
    if at_end(remaining) { return None; }
    let d = quantise(distance(top, bottom, height));
    (d > 0.).then(|| (scale(d), opacity(d)))
}

/// Which of the header's chips fit. `widths` are the chips' widths from the most to the least important, `gap` between them,
/// `room` the width the chips may have. The less important ones go first, never the order; at least the first stays.
pub fn fit(widths: &[f32], gap: f32, room: f32) -> usize {
    let (mut used, mut n) = (0., 0);
    for (i, w) in widths.iter().enumerate() {
        let add = w + if i > 0 { gap } else { 0. };
        if used + add > room && n > 0 { break; }
        used += add; n += 1;
    }
    n
}

/// What the open room keeps for the zone: the room's width, each row's height at full size (a row drawn smaller keeps that box),
/// and which rows were drawn smaller last time.
pub struct Ui { pub on: bool, pub width: f32, pub full: std::collections::HashMap<String, f32>, pub scaled: std::collections::HashSet<String>, /** the rows drawn in the last frame, for the test port */ pub last: std::collections::HashMap<usize, (f32, f32)>, /** rows' (top, height) as last drawn, the room's (width, height), and the pixels left below */ pub geo: std::collections::HashMap<usize, (f32, f32)>, pub size: (f32, f32), pub remaining: f32, /** each message's middle in window pixels as it was last painted, and the room's top (the colour fringe) */ pub mids: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, f32>>>, pub top: f32 }
impl Default for Ui { fn default() -> Ui { Ui { on: true, width: 0., full: Default::default(), scaled: Default::default(), last: Default::default(), geo: Default::default(), size: (0., 0.), remaining: 0., mids: Default::default(), top: 0. } } }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_row_in_the_band_is_whole_and_one_outside_it_shrinks_and_fades_up_to_the_limit() {
        // a 1000 px room: the band is 300..700
        assert_eq!(distance(350., 450., 1000.), 0.);
        assert_eq!(distance(250., 350., 1000.), 0.); // touches the band
        assert_eq!(distance(100., 200., 1000.), (300. - 200.) / 300.);
        assert_eq!(distance(800., 900., 1000.), (800. - 700.) / 300.);
        assert_eq!(distance(-500., -400., 1000.), 1.);
        assert_eq!(distance(0., 10., 0.), 0.);
        assert!((scale(1.) - 0.8).abs() < 1e-6 && (opacity(1.) - 0.45).abs() < 1e-6);
        assert_eq!((scale(0.), opacity(0.)), (1., 1.));
    }
    #[test]
    fn the_end_of_the_room_has_no_effect() {
        assert!(effect(0., 50., 1000., 79.).is_none());
        assert!(effect(0., 50., 1000., 80.).is_some());
        assert!(effect(400., 500., 1000., 500.).is_none());
    }
    #[test]
    fn the_distance_is_stepped_and_never_leaves_zero_to_one() {
        assert_eq!(quantise(0.01), 0.);
        assert_eq!(quantise(1.), 1.);
        for i in 0..=100 { let q = quantise(i as f32 / 100.); assert!((0. ..=1.).contains(&q)); }
    }
    #[test]
    fn the_header_drops_the_least_important_chips_first() {
        let w = [60., 70., 70., 80.];
        assert_eq!(fit(&w, 10., 1000.), 4);
        assert_eq!(fit(&w, 10., 220.), 3); // 60+10+70+10+70 = 220
        assert_eq!(fit(&w, 10., 219.), 2);
        assert_eq!(fit(&w, 10., 10.), 1); // the first always stays
        assert_eq!(fit(&[], 10., 100.), 0);
    }
}
