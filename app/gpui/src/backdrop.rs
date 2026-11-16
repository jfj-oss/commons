//! The room backdrop (design idea #1): a two-colour speckle in the room's colour and highlight. New activity sends a
//! wave out of the newest line, and where the wave passes the speckle organises into flowing bands that spread, then
//! relaxes back to speckle. Nothing moves when nothing happens: the picture is made once for the window's size and kept;
//! while a wave runs it is remade about 15 times a second, and when the last wave has faded it is made one last time.
//!
//! The picture is an opaque image (the dim room colour with the speckle already laid on it), so it is drawn the way the
//! video tiles are (`paint_image`, no blending to get wrong) and needs no GPU shader. Contrast is kept very low on
//! purpose: text lies on top of it. The battery saver (`still`) draws the settled speckle only, never a wave.
use crate::look::{self, Duo};
use gpui_kit::*;
use image::{Frame, RgbaImage};
use smallvec::smallvec;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

pub const BASE: u32 = 0x14111a;
/// Size of one speck in logical pixels.
pub const CELL: usize = 3;
/// A wave lives this long (seconds), and spreads this fast (fractions of the diagonal per second).
pub const LIFE: f64 = 7.0;
const SPEED: f64 = 0.16;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wave { pub x: f64, pub y: f64, pub t0: f64, pub s: f64 }

fn hash(x: u32, y: u32, seed: u32) -> f32 {
    let mut h = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA6B) ^ seed.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 15; h = h.wrapping_mul(0x2C1B_3C6D); h ^= h >> 12; h = h.wrapping_mul(0x297A_2D39); h ^= h >> 15;
    (h & 0xFFFF) as f32 / 65535.
}

/// How organised the speckle is at a point (0 = plain speckle, 1 = bands) and the band phase there.
pub fn order_at(waves: &[Wave], clock: f64, x: f64, y: f64, aspect: f64) -> (f32, f32) {
    let (mut o, mut phase) = (0f64, 0f64);
    for w in waves {
        let age = clock - w.t0;
        if !(0. ..LIFE).contains(&age) { continue; }
        let d = ((x - w.x) * aspect).hypot(y - w.y);
        let front = age * SPEED * aspect.max(1.);
        // a wide, soft front that carries the bands; strength fades with age, and the bands loosen at the end
        let fade = (1. - age / LIFE).powf(1.4);
        let reach = 0.10 + 0.30 * (age / LIFE);
        let e = (-((d - front) / reach).powi(2)).exp() * w.s * fade;
        // behind the front the bands go on a little, weaker
        let tail = if d < front { 0.35 * ((-(front - d) / 0.45).exp()) * w.s * fade } else { 0. };
        let k = (e + tail).min(1.);
        if k > o { o = k; phase = d * 36. - age * 3.0 + 1.6 * (x * 9. + age * 0.8).sin() * (y * 7. - age * 0.5).cos(); }
    }
    (o as f32, (0.5 + 0.5 * phase.sin()) as f32)
}

/// The picture: `w` x `h` logical pixels, as BGRA bytes (opaque). Pure, so it can be tested.
pub fn pixels(w: usize, h: usize, seed: u32, duo: Duo, waves: &[Wave], clock: f64) -> Vec<u8> {
    let bg = look::mix(BASE, duo.main, 0.04);
    let c_main = if look::contrast(duo.main, bg) < 1.6 { look::mix(duo.main, duo.hi, 0.4) } else { duo.main };
    let c_hi = duo.hi;
    let (cols, rows) = (w.div_ceil(CELL), h.div_ceil(CELL));
    let aspect = w as f64 / h.max(1) as f64;
    let mut out = vec![0u8; w * h * 4];
    let bgc = [(bg >> 16) as f32, ((bg >> 8) & 255) as f32, (bg & 255) as f32];
    for cy in 0..rows {
        for cx in 0..cols {
            let r = hash(cx as u32, cy as u32, seed);
            let r2 = hash(cx as u32 + 7919, cy as u32 + 104729, seed);
            let (o, band) = if waves.is_empty() { (0., 0.) } else { order_at(waves, clock, (cx as f64 + 0.5) / cols as f64, (cy as f64 + 0.5) / rows as f64, aspect) };
            // the colour a speck is: random in plain speckle, the band's phase where the wave has organised it
            let pick = r * (1. - o) + band * o;
            let col = if pick > 0.5 { c_hi } else { c_main };
            // a faint tone-on-tone: most specks barely show, a few more (kept low: text lies on this)
            let a = (0.035 + 0.075 * r2 * r2) * (1. + 1.9 * o);
            let cc = [(col >> 16) as f32, ((col >> 8) & 255) as f32, (col & 255) as f32];
            let px = [(bgc[2] + (cc[2] - bgc[2]) * a) as u8, (bgc[1] + (cc[1] - bgc[1]) * a) as u8, (bgc[0] + (cc[0] - bgc[0]) * a) as u8, 255];
            for y in cy * CELL..((cy + 1) * CELL).min(h) {
                let row = y * w * 4;
                for x in cx * CELL..((cx + 1) * CELL).min(w) { out[row + x * 4..row + x * 4 + 4].copy_from_slice(&px); }
            }
        }
    }
    out
}

pub fn render_image(w: usize, h: usize, seed: u32, duo: Duo, waves: &[Wave], clock: f64) -> Arc<RenderImage> {
    let img = RgbaImage::from_raw(w as u32, h as u32, pixels(w, h, seed, duo, waves, clock)).expect("size");
    Arc::new(RenderImage::new(smallvec![Frame::new(img)]))
}

/// What the canvas remembers between frames: the last picture and what it was made for.
#[derive(Default)]
pub struct Cache { key: Option<(usize, usize, u32, u32, u32, i64)>, img: Option<Arc<RenderImage>>, pub made: u64 }
pub type Shared = Rc<RefCell<Cache>>;

/// The element: fills its parent. `still`: the settled speckle only (battery saver).
pub fn view(cache: Shared, seed: u32, duo: Duo, waves: Vec<Wave>, clock: f64, still: bool) -> impl IntoElement {
    canvas(|_, _, _| (), move |bounds, _, window, _| {
        let (w, h) = (f32::from(bounds.size.width).round() as usize, f32::from(bounds.size.height).round() as usize);
        if w == 0 || h == 0 { return; }
        let live = !still && waves.iter().any(|v| (0. ..LIFE).contains(&(clock - v.t0)));
        // 15 pictures a second while a wave runs; one picture when there is none
        let bucket = if live { (clock * 15.) as i64 } else { -1 };
        let key = (w, h, seed, duo.main, duo.hi, bucket);
        let mut c = cache.borrow_mut();
        if c.key != Some(key) {
            let new = render_image(w, h, seed, duo, if live { &waves } else { &[] }, clock);
            if let Some(old) = c.img.replace(new) { let _ = window.drop_image(old); }
            c.key = Some(key);
            c.made += 1;
        }
        if let Some(i) = c.img.clone() { let _ = window.paint_image(bounds, bounds, Corners::default(), i, 0, false); }
    }).size_full()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn duo() -> Duo { Duo { main: 0x6a35ff, hi: 0xffb3f0 } }
    fn spread(px: &[u8]) -> i32 { let (mut lo, mut hi) = (255i32, 0i32); for c in px.chunks(4) { let v = c[0] as i32 + c[1] as i32 + c[2] as i32; lo = lo.min(v); hi = hi.max(v); } hi - lo }

    #[::core::prelude::v1::test]
    fn plain_speckle_is_deterministic_and_faint() {
        let a = pixels(90, 60, 5, duo(), &[], 0.);
        assert_eq!(a, pixels(90, 60, 5, duo(), &[], 99.), "no wave: the clock changes nothing");
        assert_ne!(a, pixels(90, 60, 6, duo(), &[], 0.), "another room, another speckle");
        assert!(a.chunks(4).all(|p| p[3] == 255));
        // very low contrast behind text: no speck is further than a small step from the base colour
        let base = look::mix(BASE, duo().main, 0.04);
        let (br, bg, bb) = ((base >> 16) as i32, ((base >> 8) & 255) as i32, (base & 255) as i32);
        for p in a.chunks(4) { assert!((p[2] as i32 - br).abs() < 46 && (p[1] as i32 - bg).abs() < 46 && (p[0] as i32 - bb).abs() < 46); }
    }
    #[::core::prelude::v1::test]
    fn a_wave_organises_the_speckle_and_then_it_relaxes() {
        let wv = [Wave { x: 0.1, y: 0.9, t0: 0., s: 1. }];
        let (o_mid, _) = order_at(&wv, 2.0, 0.1 + 0.16 * 2.0 * 1.5, 0.9, 1.5);
        assert!(o_mid > 0.3, "the front is organised: {o_mid}");
        let (o_far, _) = order_at(&wv, 2.0, 0.99, 0.0, 1.5);
        assert!(o_far < 0.05, "far ahead of the front nothing yet: {o_far}");
        let (o_late, _) = order_at(&wv, LIFE + 0.1, 0.5, 0.5, 1.5);
        assert_eq!(o_late, 0.);
        // and the picture differs while it runs, and is the plain speckle again when it has gone
        let plain = pixels(120, 80, 3, duo(), &[], 0.);
        assert_ne!(plain, pixels(120, 80, 3, duo(), &wv, 2.0));
        assert_eq!(plain, pixels(120, 80, 3, duo(), &wv, LIFE + 1.0));
        assert!(spread(&plain) > 0);
    }
}
