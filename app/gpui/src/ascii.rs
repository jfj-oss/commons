//! The live ASCII fields of the current look (app/ui/mega.js: RAMP, FX, asciiDraw, bdDraw), ported cell for cell.
//!
//! A field is a grid of monospace cells. Each cell asks its pattern for a value 0..1 at the field's clock, picks a
//! glyph from the ramp by that value and one of three tints between the room's two colours. Here every cell is one
//! glyph sprite from GPUI's glyph atlas (`Window::paint_glyph`), so the GPU draws the field as instanced quads; nothing
//! is rasterised per frame.
use crate::look;
use gpui_kit::*;

pub const RAMP: &str = " .:-=+*#%@";
#[allow(dead_code)] // the chooser of a later step lists them
pub const FX_NAMES: [&str; 17] = ["waves", "ripples", "rain", "clouds", "spiral", "lattice", "haze", "aurora", "tide", "tunnel", "orbit", "snow", "embers", "radar", "weave", "comets", "dunes"];

fn nh(x: f64, y: f64) -> f64 { let s = (x * 127.1 + y * 311.7).sin() * 43758.5453; s - s.floor() }
fn vn(x: f64, y: f64) -> f64 {
    let (i, j) = (x.floor(), y.floor());
    let (f, g) = (x - i, y - j);
    let (u, w) = (f * f * (3. - 2. * f), g * g * (3. - 2. * g));
    (nh(i, j) * (1. - u) + nh(i + 1., j) * u) * (1. - w) + (nh(i, j + 1.) * (1. - u) + nh(i + 1., j + 1.) * u) * w
}

/// Per-column randomness for rain and snow (mega.js `colFor`).
#[derive(Clone, Copy)]
pub struct Col { v: f64, o: f64, l: f64 }
pub fn cols_for(n: usize, seed: u32) -> Vec<Col> {
    let s = seed as f64;
    (0..n.max(1)).map(|i| { let i = i as f64; Col { v: 0.6 + nh(i, s) * 1.5, o: nh(s, i) * 60., l: 16. + nh(i + 9., s) * 30. } }).collect()
}

/// Pattern `n` at cell (x, y) and time t, in a field of cols x rows. The numbers are mega.js FX, in the same order:
/// an owner's chosen pattern is an index into this list on every device.
pub fn fx(n: usize, x: f64, y: f64, t: f64, cols: f64, rows: f64, col: &[Col]) -> f64 {
    let c = col[(x as usize) % col.len()];
    let clamp = |v: f64| v.clamp(0., 1.);
    match n {
        0 => 0.5 + 0.5 * (x * 0.11 + t + (y * 0.09 - t * 0.6).sin() * 2.2).sin() * (y * 0.16 - t * 0.4).cos(),
        1 => { let (cx, cy) = (cols * (0.5 + 0.3 * (t * 0.31).sin()), rows * (0.5 + 0.35 * (t * 0.23).cos())); let d = (x - cx).hypot((y - cy) * 1.9); 0.5 + 0.5 * (d * 0.34 - t * 1.6).sin() }
        2 => { let p = ((y + t * c.v * 7. + c.o) % c.l) / c.l; if p > 0.4 { ((p - 0.4) / 0.6).powf(1.6) } else { 0. } }
        3 => clamp((vn(x * 0.05 + t * 0.1, y * 0.11 - t * 0.06) * 0.65 + vn(x * 0.14 - t * 0.15, y * 0.28) * 0.35 - 0.22) / 0.56),
        4 => { let (dx, dy) = (x - cols / 2., (y - rows / 2.) * 1.9); 0.5 + 0.5 * (dy.atan2(dx) * 5. + (1. + dx.hypot(dy)).ln() * 4. - t * 1.4).sin() }
        5 => 0.5 + 0.5 * ((x + y * 1.9) * 0.18 + t).sin() * ((x - y * 1.9) * 0.14 - t * 0.7).sin(),
        6 => 0.34 * (0.5 + 0.5 * (x * 0.07 + y * 0.19 + t * 0.3).sin()) + 0.1,
        7 => { let c = 0.5 + 0.5 * (x * 0.13 + (t * 0.4 + x * 0.045).sin() * 2.5 + t * 0.3).sin(); c * c * (0.55 + 0.45 * (y * 0.08 - t * 0.5 + x * 0.02).sin()) }
        8 => 0.5 + 0.5 * (y * 0.5 - t * 1.2 + (x * 0.05 + t * 0.5).sin() * 1.4).sin(),
        9 => { let (dx, dy) = ((x - cols / 2.).abs() / (cols / 2.), (y - rows / 2.).abs() / (rows / 2.)); 0.5 + 0.5 * (dx.max(dy) * 15. + t * 2.).sin() }
        10 => {
            let mut s = 0.;
            for i in 0..3 { let i = i as f64; let (dx, dy) = (x - cols * (0.5 + 0.27 * (t * 0.5 + i * 2.1).cos()), (y - rows * (0.5 + 0.3 * (t * 0.7 + i * 2.1).sin())) * 1.9); s += 70. / (dx * dx + dy * dy + 1.); }
            clamp((s - 0.3) * 1.3)
        }
        11 => { let xx = x + ((t * 0.5 + y * 0.3 + c.o).sin() * 1.5).round(); let n = nh(xx * 1.7, (y - t * (1.2 + c.v)).floor() + c.o); if n > 0.93 { 0.55 + (n - 0.93) * 6. } else { 0. } }
        12 => { let f = y / rows; clamp((vn(x * 0.11, y * 0.27 - t * 1.3) * 1.25 - 0.22) * f * f * 1.7) }
        13 => { let tau = 6.2832; let d = ((t * 1.1 - ((y - rows / 2.) * 1.9).atan2(x - cols / 2.)) % tau + tau) % tau; if d < 3. { (1. - d / 3.) * (1. - d / 3.) } else { 0. } }
        14 => { let p = 0.5 + 0.5 * ((x + y * 1.9) * 0.35 + t * 0.6).sin(); let q = 0.5 + 0.5 * ((x - y * 1.9) * 0.35 - t * 0.5).sin(); ((if p > 0.8 { p } else { 0. }) + (if q > 0.8 { q } else { 0. })).min(1.) }
        15 => {
            let w = x + y * 1.9; let lane = (w / 5.).floor(); let h = nh(lane, 3.7);
            if h < 0.35 || w - lane * 5. > 2.2 { return 0.; }
            let l = x * 1.9 - y; let d = (((t * (10. + h * 14.) + h * 120. - l) % 130.) + 130.) % 130.;
            if d < 24. { (1. - d / 24.) * (1. - d / 24.) } else { 0. }
        }
        _ => { let r = 1. - (vn(x * 0.045 + t * 0.03, y * 0.16) * 2. - 1.).abs(); clamp((r * r * r - 0.3) / 0.7) }
    }
}

/// The pattern a card gets when its owner chose none (mega.js `auto`).
pub fn auto_fx(name: &str) -> usize { [4, 3, 0, 5, 2][(look::h(name) % 5) as usize] }

/// A ring that travels outward through the backdrop (mega.js `ripple`): from a new message, or now and then by itself.
#[derive(Clone, Copy, Debug)]
pub struct Ripple { pub x: f64, pub y: f64, pub r: f64, pub s: f64, pub v: f64, pub band: f64, pub fade: f64, /** x and y are fractions of the field's columns and rows, not cells */ pub frac: bool }

/// A drop of water falling into the backdrop (backdrop.rs `Bd::drop_in`): `t` seconds since it started to fall. It falls for
/// `DROP_FALL` seconds as a short bright streak, then a crown of spray jumps up from where it landed and falls back; the rings
/// it leaves are ordinary `Ripple`s that the backdrop's clock sends out.
#[derive(Clone, Copy, Debug)]
pub struct Drop { /** where it lands, as fractions of the columns and rows */ pub x: f64, pub y: f64, pub t: f64, pub s: f64, pub seed: u32 }
pub const DROP_FALL: f64 = 0.55;
/// How long the spray is in the air after the landing.
pub const SPRAY: f64 = 0.9;

/// The cells a drop lights this frame, in cells of a cols x rows field, each with its brightness.
pub fn drop_cells(d: &Drop, cols: f64, rows: f64) -> Vec<(i64, i64, f64)> {
    let (lx, ly) = (d.x * cols, d.y * rows);
    let mut out = vec![];
    if d.t < DROP_FALL {
        // the fall: from about a third of the room above, speeding up (p squared), with a short fading tail
        let p = d.t / DROP_FALL;
        let fall = (rows * 0.35).max(6.);
        let head = ly - fall * (1. - p * p);
        let tail = 1. + 3. * p;
        for i in 0..=(tail as i64) { out.push((lx.round() as i64, (head - i as f64).round() as i64, d.s * (1. - i as f64 / (tail + 1.)))); }
        return out;
    }
    let a = d.t - DROP_FALL;
    // the splash: the landing cell flashes, then a crown of spray flies up and out and falls back under gravity
    if a < 0.22 { let f = d.s * (1. - a / 0.22); for (dx, dy) in [(0, 0), (-1, 0), (1, 0)] { out.push((lx.round() as i64 + dx, ly.round() as i64 + dy, f)); } }
    if a < SPRAY {
        let n = 7;
        for i in 0..n {
            let h = |k: f64| { let v = ((d.seed as f64) * 0.37 + i as f64 * 12.9898 + k * 78.233).sin() * 43758.5453; v - v.floor() };
            let side = if i % 2 == 0 { 1. } else { -1. };
            let (vx, vy) = (side * (2.5 + h(1.) * 7.), -(7. + h(2.) * 7.));   // cells a second; up is negative
            let (x, y) = (lx + vx * a, ly + vy * a + 0.5 * 26. * a * a);
            if y > ly + 0.5 { continue; }   // back in the water
            out.push((x.round() as i64, y.round() as i64, d.s * 0.9 * (1. - a / SPRAY)));
        }
    }
    out
}

#[derive(Clone)]
pub struct Field {
    pub fx: usize,
    pub seed: u32,
    pub duo: look::Duo,
    pub t: f64,
    /// Cell size and font size in pixels (mega.js: 10 x 17 at 14 px, or 8 x 13 at 11 px on a small card).
    pub cell: (f32, f32),
    pub font_px: f32,
    /// Below this nothing is drawn; above `hi_t` the full highlight colour.
    pub gate: f64,
    pub hi_t: f64,
    pub alpha: f32,
    /// Corner radius of the squircle the field lies on: cells outside its corners are left out.
    pub radius: f32,
    pub ripples: Vec<Ripple>,
    /// Drops of water falling in (the room's backdrop only).
    pub drops: Vec<Drop>,
    /// Boxes (relative to the field) the pattern flows around: the words on a card.
    pub holes: Vec<Bounds<Pixels>>,
    /// The same, in window coordinates, gathered while this frame was laid out: the words of a chat (see `hole`).
    pub live_holes: Option<Holes>,
    /// Battery saver: the field holds still as a faint tone-on-tone emboss in the card's own colour (a light edge and a dark one)
    /// instead of its two colours.
    pub emboss: bool,
    /// A drawing on the card (wall.rs): its grid, and the cells that hold a mark. The field's own ASCII leaves those cells alone.
    pub avoid: Option<(crate::wall::Grid, std::rc::Rc<Vec<(u16, u8, u8)>>)>,
    /// The three tints (soft, mid, bright) when they are not simply the duo mixed 0.3, 0.6, 1 (the room's backdrop, mega.js `bdColours`), and the brighter three under a ripple.
    pub tints: Option<[Hsla; 3]>,
    pub lit: Option<[Hsla; 3]>,
    /// A soft glow under the pointer on the focused card (mega.js `bloom`): the pointer's cell, and how strong it still is (1 right after a move, then it fades).
    pub bloom: Option<(f64, f64, f64)>,
}

/// Boxes the backdrop must leave free, filled in during a frame's layout pass and read when the field is painted after it.
pub type Holes = std::rc::Rc<std::cell::RefCell<Vec<Bounds<Pixels>>>>;

/// Put inside a (relative) element: the field behind flows around that element's box.
pub fn hole(h: &Holes) -> impl IntoElement {
    let h = h.clone();
    canvas(move |b, _, _| h.borrow_mut().push(b), |_, _, _, _| {}).absolute().top_0().left_0().size_full()
}

impl Field {
    pub fn card(fx: usize, seed: u32, duo: look::Duo, t: f64, small: bool) -> Field {
        Field { fx, seed, duo, t, cell: if small { (8., 13.) } else { (10., 17.) }, font_px: if small { 11. } else { 14. }, gate: 0.2, hi_t: 0.9, alpha: 1., radius: 0., ripples: vec![], drops: vec![], holes: vec![], live_holes: None, emboss: false, avoid: None, tints: None, lit: None, bloom: None }
    }
}

/// A text box in window coordinates as a box relative to the field, a cell wider on each side (mega.js `hit`: x0 - 1 .. x1 + 1).
pub fn pad_box(b: &Bounds<Pixels>, origin: Point<Pixels>, cw: f32) -> Bounds<Pixels> {
    Bounds::new(point(b.origin.x - origin.x - px(cw), b.origin.y - origin.y), size(b.size.width + px(2. * cw), b.size.height))
}

/// Does the cell at (x, y) of size cw x ch touch the box? Such a cell is cut out of the pattern.
pub fn cell_in_box(x: f32, y: f32, cw: f32, ch: f32, b: &Bounds<Pixels>) -> bool {
    x + cw > f32::from(b.origin.x) && x < f32::from(b.origin.x + b.size.width) && y + ch > f32::from(b.origin.y) && y < f32::from(b.origin.y + b.size.height)
}

#[cfg(test)]
mod hole_tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn a_text_box_is_cut_out_a_cell_wider_each_side() {
        // a word at x 100..180, y 50..70 inside a field whose window origin is (20, 10): relative 80..160, 40..60
        let b = Bounds::new(point(px(100.), px(50.)), size(px(80.), px(20.)));
        let r = pad_box(&b, point(px(20.), px(10.)), 10.);
        assert_eq!((f32::from(r.origin.x), f32::from(r.origin.y), f32::from(r.size.width), f32::from(r.size.height)), (70., 40., 100., 20.));
        let (cw, ch) = (10., 17.);
        assert!(cell_in_box(70., 34., cw, ch, &r), "the padding column left of the word is cut");
        assert!(cell_in_box(160., 51., cw, ch, &r), "and the one right of it");
        assert!(!cell_in_box(50., 40., cw, ch, &r), "two cells left is pattern again");
        assert!(!cell_in_box(80., 57. + 3., cw, ch, &r), "the row below the box is pattern");
        assert!(!cell_in_box(80., 23., cw, ch, &r), "and the row above");
        assert!(cell_in_box(80., 24., cw, ch, &r), "a row that touches the box by a hair is cut");
    }
}

struct Ramp { font: FontId, glyphs: Vec<Option<GlyphId>> }

fn ramp(window: &mut Window) -> Ramp {
    let ts = window.text_system().clone();
    let f = font(look::F_MONO);
    let id = ts.resolve_font(&f);
    // one shaped glyph per ramp character (the first is a space: nothing to draw)
    let glyphs = RAMP.chars().map(|c| {
        if c == ' ' { return None; }
        let s: SharedString = c.to_string().into();
        let run = TextRun { len: s.len(), font: f.clone(), color: gpui_kit::black(), background_color: None, underline: None, strikethrough: None };
        let line = ts.shape_line(s, px(14.), &[run], None);
        line.runs.first().and_then(|r| r.glyphs.first()).map(|g| g.id)
    }).collect();
    Ramp { font: id, glyphs }
}

/// The element: fills its parent.
pub fn field(f: Field) -> impl IntoElement {
    canvas(|_, _, _| (), move |bounds, _, window, _cx| {
        let r = ramp(window);
        let (cw, ch) = f.cell;
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let (cols, rows) = ((w / cw).ceil() as usize, (h / ch).ceil() as usize);
        if cols == 0 || rows == 0 { return; }
        let col = cols_for(cols, f.seed);
        let tint = |k: f32| -> Hsla { let mut c: Hsla = rgb(look::mix(f.duo.main, f.duo.hi, k)).into(); c.a = f.alpha; c };
        let tints = f.tints.unwrap_or([tint(0.3), tint(0.6), tint(1.0)]);
        // the glyph sits in the middle of its cell: a monospace advance is 0.6 em, and the baseline about 0.72 of the cell down
        let (ox, oy) = ((cw - f.font_px * 0.6) / 2., ch * 0.72);
        // this frame's words, relative to the field, a cell wider on each side (as mega.js `hit` does)
        let mut holes = f.holes.clone();
        if let Some(live) = &f.live_holes {
            holes.extend(live.borrow().iter().filter(|b| b.intersects(&bounds)).map(|b| pad_box(b, bounds.origin, cw)));
        }
        // where the drawing is: cells of this field that the pattern must leave alone (mega-wall.js `cardMask`)
        let avoid = f.avoid.as_ref().map(|(g, m)| crate::wall::field_mask(g, m.iter().map(|c| c.0), cw, ch, cols, rows));
        let rad = f.radius;
        // cells outside the squircle's corners are left out (the card's own cut corners then only have to tidy the edge)
        let outside_corner = |x: f32, y: f32| -> bool { rad > 0. && !crate::squircle::contains(x, y, 0., 0., w, h, rad) };
        // the drops' bright cells this frame (a handful at most)
        let mut wet: std::collections::HashMap<(i64, i64), f64> = Default::default();
        for d in &f.drops { for (x, y, s) in drop_cells(d, cols as f64, rows as f64) { let e = wet.entry((x, y)).or_default(); *e = e.max(s); } }
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for y in 0..rows {
                for x in 0..cols {
                    let (px_x, px_y) = (x as f32 * cw, y as f32 * ch);
                    if outside_corner(px_x + cw / 2., px_y + ch / 2.) { continue; }
                    if avoid.as_ref().is_some_and(|m| m[y * cols + x]) { continue; }
                    if holes.iter().any(|b| cell_in_box(px_x, px_y, cw, ch, b)) { continue; }
                    let (xf, yf) = (x as f64, y as f64);
                    let mut v = fx(f.fx, xf, yf, f.t, cols as f64, rows as f64, &col);
                    let mut lit = 0f64;
                    if let Some((mx, my, b)) = f.bloom { let (dx, dy) = (xf - mx, (yf - my) * 1.8); v += b * (-(dx * dx + dy * dy) / 90.).exp() * 0.85; }
                    for q in &f.ripples {
                        let (qx, qy) = if q.frac { (q.x * cols as f64, q.y * rows as f64) } else { (q.x, q.y) };
                        let d = (xf - qx).hypot((yf - qy) * 1.7) - q.r;
                        if d * d < q.band * 5. { let e = (-d * d / q.band).exp() * q.s; v += e; lit = lit.max(e); }
                    }
                    if let Some(&s) = wet.get(&(x as i64, y as i64)) { v = v.max(0.3) + s; lit = lit.max(s); }
                    if v < f.gate { continue; }
                    let Some(g) = r.glyphs[((v * 10.) as usize).min(9)] else { continue };
                    if f.emboss {
                        // tone on tone: the same glyph twice, a hair apart, a little lighter and a little darker than the card
                        let strength = 0.05 + 0.10 * v as f32;
                        let mut dark: Hsla = rgb(look::mix(f.duo.main, 0x000000, 0.22 + strength)).into();
                        let mut light: Hsla = rgb(look::mix(f.duo.main, 0xffffff, 0.10 + strength)).into();
                        dark.a = 0.5; light.a = 0.5;
                        let p = point(bounds.origin.x + px(px_x + ox), bounds.origin.y + px(px_y + oy));
                        let _ = window.paint_glyph(point(p.x - px(0.7), p.y - px(0.7)), r.font, g, px(f.font_px), dark);
                        let _ = window.paint_glyph(point(p.x + px(0.7), p.y + px(0.7)), r.font, g, px(f.font_px), light);
                        continue;
                    }
                    let k = if v < 0.64 { 0 } else if v < f.hi_t { 1 } else { 2 };
                    let mut color = tints[k];
                    if lit > 0.2 { match &f.lit { Some(l) => color = l[k], None => color.a = (color.a * 1.8).min(1.) } }
                    let _ = window.paint_glyph(point(bounds.origin.x + px(px_x + ox), bounds.origin.y + px(px_y + oy)), r.font, g, px(f.font_px), color);
                }
            }
        });
    }).size_full()
}
