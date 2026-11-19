//! Text turned a quarter circle, reading upward: the call dock's room name (mega.css `.cd-room { writing-mode: vertical-rl;
//! transform: rotate(180deg) }`). GPUI cannot rotate text, so the glyph outlines of the same FK Raster Sharp file the window
//! loads are read with ttf-parser and filled as paths.
use gpui_kit::*;

const SHARP: &[u8] = include_bytes!("../../ui/fonts/FKRasterRomanCompactTrial-Sharp.otf");

struct Pen { pb: PathBuilder, at: (f32, f32), k: f32, ox: f32, oy: f32, last: (f32, f32), open: bool }
impl Pen {
    /// Font units (x along the line, y up) to the screen: the line runs upward, the glyphs' tops point left.
    fn map(&self, x: f32, y: f32) -> Point<Pixels> { point(px(self.ox - y * self.k), px(self.oy - (self.at.0 + x) * self.k)) }
}
impl ttf_parser::OutlineBuilder for Pen {
    fn move_to(&mut self, x: f32, y: f32) { let p = self.map(x, y); self.pb.move_to(p); self.last = (x, y); self.open = true; }
    fn line_to(&mut self, x: f32, y: f32) { let p = self.map(x, y); self.pb.line_to(p); self.last = (x, y); }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) { let (c, p) = (self.map(x1, y1), self.map(x, y)); self.pb.curve_to(p, c); self.last = (x, y); }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) { let (a, b, p) = (self.map(x1, y1), self.map(x2, y2), self.map(x, y)); self.pb.cubic_bezier_to(p, a, b); self.last = (x, y); }
    fn close(&mut self) { self.pb.close(); self.open = false; }
}

/// How far the text runs, in px, at `size` (the font's own advances, `track` em of extra space after each letter).
pub fn length(text: &str, size: f32, track: f32) -> f32 {
    let Ok(face) = ttf_parser::Face::parse(SHARP, 0) else { return 0. };
    let k = size / face.units_per_em() as f32;
    text.chars().map(|c| face.glyph_index(c).and_then(|g| face.glyph_hor_advance(g)).unwrap_or(0) as f32 * k + track * size).sum()
}

/// Paints `text` with its baseline on the vertical line `x_base`, starting at `y_start` and running up for at most `max_len` px.
pub fn paint(window: &mut Window, text: &str, size: f32, track: f32, x_base: f32, y_start: f32, max_len: f32, color: Hsla) {
    let Ok(face) = ttf_parser::Face::parse(SHARP, 0) else { return };
    let k = size / face.units_per_em() as f32;
    let mut adv = 0f32; // in font units
    for c in text.chars() {
        let Some(g) = face.glyph_index(c) else { adv += size * 0.3 / k; continue };
        let w = face.glyph_hor_advance(g).unwrap_or(0) as f32;
        if (adv + w) * k > max_len { break; }
        let mut pen = Pen { pb: PathBuilder::fill(), at: (adv, 0.), k, ox: x_base, oy: y_start, last: (0., 0.), open: false };
        if face.outline_glyph(g, &mut pen).is_some() { if let Ok(p) = pen.pb.build() { window.paint_path(p, color); } }
        adv += w + track * size / k;
    }
}
