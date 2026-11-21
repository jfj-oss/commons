//! The current mega look: the dark void, paper ink, a room's two colours, and the faces (app/ui/mega.css, mega.js).
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;

pub const VOID: u32 = 0x050407;
pub const PAPER: u32 = 0xefedf2;
pub const MUTE: u32 = 0xa39fae;
pub const SLIP: u32 = 0x100e16;
pub const SPEAK: u32 = 0x4df0c0;
pub const DANGER: u32 = 0xff6b62;

/// Titles and numerals.
pub const F_SHARP: &str = "FK Raster Roman Compact Trial Sharp";
/// Titles while a card moves (the page swaps to Sharp when it settles).
pub const F_BLEND: &str = "FK Raster Roman Compact Trial Blended";
/// Message text (the "Raster" chat font).
pub const F_MSG: &str = "FK Raster Roman Compact Trial Rounded";
/// Names.
pub const F_NAME: &str = "FK Raster Grotesk Trial Rounded";
/// Message text (the default chat font of the web UI).
pub const F_BELLEFAIR: &str = "Bellefair";
pub const F_UI: &str = "Figtree Light";
pub const F_MONO: &str = "IBM Plex Mono Medium";

pub fn line() -> Hsla { hsla(0., 0., 1., 0.12) }
pub fn paper() -> Hsla { rgb(PAPER).into() }
pub fn mute() -> Hsla { rgb(MUTE).into() }

pub fn apply_theme(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    Theme::update(cx, |t| {
        t.background = rgb(VOID).into();
        t.foreground = rgb(PAPER).into();
        t.muted_foreground = rgb(MUTE).into();
        t.border = line();
        t.input = line();
        t.ring = rgb(SPEAK).into();
        t.caret = rgb(PAPER).into();
        t.primary = rgb(PAPER).into();
        t.primary_hover = rgb(0xffffff).into();
        t.primary_active = rgb(0xd8d5de).into();
        t.primary_foreground = rgb(0x0c0d12).into();
        t.font_family = F_UI.into();
        // the window frame (titlebar.rs): Root draws a 1 px light edge of this colour at this radius, outside mega's dark band
        t.window_border = crate::titlebar::edge_colour(true);
        t.tokens.background = crate::titlebar::band_colour(true).into();
        t.window_radius = px(crate::titlebar::R_EDGE);
    });
}

// ---- colours per room (mega.js: DEF, targetOf, dotOf, bestInk) ----
pub const DEF: [(u32, u32); 12] = [(0xff5a1f, 0xffe14d), (0x101640, 0x45f0d0), (0xf3c6d8, 0xc2124a), (0x0b3d2e, 0xd8ff3e), (0xe8e4d8, 0x1b33ff), (0x1a1a1a, 0xff2e63),
    (0x6a35ff, 0xffb3f0), (0x00a6a6, 0xfff275), (0xffd23f, 0x5a189a), (0x2d6a4f, 0xffb703), (0xe63946, 0xf1faee), (0x264653, 0xe9c46a)];

/// mega.js `H`: the hash that picks a room's default colours and pattern, so both UIs agree.
pub fn h(s: &str) -> u32 { s.chars().fold(7u32, |a, c| { let mut b = [0u16; 2]; a.wrapping_mul(31).wrapping_add(c.encode_utf16(&mut b)[0] as u32) }) }

pub fn parse_hex(s: &str) -> Option<u32> { u32::from_str_radix(s.strip_prefix('#')?, 16).ok() }

/// Bold palettes an owner can pick for a room (design idea #6), as (name, colour, highlight).
pub const PRESETS: [(&str, u32, u32); 3] = [("violet and orange", 0x6a35ff, 0xff8c1a), ("cobalt and white", 0x1b33ff, 0xffffff), ("lavender and green", 0xc9b8ff, 0x1f9d5c)];
pub fn hex(c: u32) -> String { format!("#{c:06x}") }

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Duo { pub main: u32, pub hi: u32 }

impl Duo {
    /// The owner's choice if there is one, else the default for this id.
    pub fn of(id: &str, chosen: Option<&(String, String)>) -> Duo {
        if let Some((m, hi)) = chosen { if let (Some(m), Some(hi)) = (parse_hex(m), parse_hex(hi)) { return Duo { main: m, hi }; } }
        let (main, hi) = DEF[(h(id) as usize) % DEF.len()];
        Duo { main, hi }
    }
    pub fn field(&self) -> Hsla { rgb(self.main).into() }
    /// Ink that reads on the field.
    pub fn ink(&self) -> Hsla { if contrast(self.main, 0x0c0d12) >= contrast(self.main, 0xffffff) { rgb(0x0c0d12).into() } else { rgb(0xffffff).into() } }
    /// The dot on the dark rail: the field if it shows on the void, else the highlight brightened until it does.
    pub fn dot(&self) -> Hsla {
        if contrast(self.main, VOID) >= 3.4 { return rgb(self.main).into(); }
        let mut c = self.hi;
        for _ in 0..12 { if contrast(c, VOID) >= 3.4 { break; } c = mix(c, 0xffffff, 0.14); }
        rgb(c).into()
    }
}

fn ch(c: u32) -> [f32; 3] { [((c >> 16) & 255) as f32, ((c >> 8) & 255) as f32, (c & 255) as f32] }
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let (x, y) = (ch(a), ch(b));
    let m = |i: usize| ((x[i] + (y[i] - x[i]) * t).round().clamp(0., 255.)) as u32;
    (m(0) << 16) | (m(1) << 8) | m(2)
}
fn lum(c: u32) -> f32 {
    let f = |v: f32| { let v = v / 255.; if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) } };
    let [r, g, b] = ch(c);
    0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
}
/// mega.js `inkOn`: the dark ink or white, whichever reads on the colour.
pub fn ink_on(c: u32) -> Hsla { if contrast(c, 0x0c0d12) >= contrast(c, 0xffffff) { rgb(0x0c0d12).into() } else { rgb(0xffffff).into() } }
pub fn contrast(a: u32, b: u32) -> f32 { let (x, y) = (lum(a), lum(b)); (x.max(y) + 0.05) / (x.min(y) + 0.05) }

/// A person's colour (mega.js `faceColor`): hsl(H(name) % 360, 78%, 74%).
pub fn face(name: &str) -> Hsla { hsla((h(name) % 360) as f32 / 360., 0.78, 0.74, 1.) }

// ---- the colour picker's maths (mega.js: toHsv, fromHsv, lift, hexes) ----
/// The eight presets of the colours panel: the first eight of the default pairs (mega.js `PRESETS`).
pub fn presets() -> [(u32, u32); 8] { let mut o = [(0, 0); 8]; o.copy_from_slice(&DEF[..8]); o }
/// A `#rrggbb` that parses (what an owner's style may hold).
pub fn is_hex(s: &str) -> bool { s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit()) }
/// A colour typed in the hex box: `#rrggbb`, or `#rgb` (each digit doubled).
pub fn parse_any_hex(s: &str) -> Option<u32> {
    let d = s.strip_prefix('#')?;
    if !d.bytes().all(|b| b.is_ascii_hexdigit()) { return None; }
    match d.len() { 6 => u32::from_str_radix(d, 16).ok(), 3 => { let e: String = d.chars().flat_map(|c| [c, c]).collect(); u32::from_str_radix(&e, 16).ok() } _ => None }
}
/// Hue (0..360), saturation and value (0..1) of a colour.
pub fn to_hsv(c: u32) -> [f32; 3] {
    let [r, g, b] = ch(c).map(|v| v / 255.);
    let mx = r.max(g).max(b); let d = mx - r.min(g).min(b);
    let mut h = 0.;
    if d > 0. { h = if mx == r { ((g - b) / d + 6.) % 6. } else if mx == g { (b - r) / d + 2. } else { (r - g) / d + 4. }; }
    [h * 60., if mx > 0. { d / mx } else { 0. }, mx]
}
pub fn from_hsv(h: f32, s: f32, v: f32) -> u32 {
    let f = |n: f32| { let k = (n + h / 60.) % 6.; (v - v * s * k.min(4. - k).min(1.).max(0.)) * 255. };
    let q = |x: f32| x.round().clamp(0., 255.) as u32;
    (q(f(5.)) << 16) | (q(f(3.)) << 8) | q(f(1.))
}
/// mega.js `lift`: brighten a colour until it reads at `ratio` on the dark backdrop.
pub fn lift(c: u32, ratio: f32) -> u32 { let mut c = c; let mut i = 0; while contrast(c, 0x050407) < ratio && i < 12 { c = mix(c, 0xffffff, 0.14); i += 1; } c }
/// The two inks a drawing on a card uses (mega.js `wallInk`): the card's ink and its highlight (or the other ink where the highlight does not show).
pub fn wall_ink(d: &Duo) -> [u32; 2] {
    let ink = if contrast(d.main, 0x0c0d12) >= contrast(d.main, 0xffffff) { 0x0c0d12 } else { 0xffffff };
    let alt = if ink == 0x0c0d12 { 0xffffff } else { 0x0c0d12 };
    [ink, if contrast(d.hi, d.main) >= 2.4 { d.hi } else { alt }]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn hsv_round_trips() {
        for c in [0xff5a1f, 0x101640, 0x00a6a6, 0xffffff, 0x000000, 0x6a35ff] {
            let [h, s, v] = to_hsv(c);
            let back = from_hsv(h, s, v);
            for sh in [16, 8, 0] { assert!((((c >> sh) & 255) as i32 - ((back >> sh) & 255) as i32).abs() <= 1, "{c:06x} -> {back:06x}"); }
        }
        assert_eq!(from_hsv(0., 1., 1.), 0xff0000);
        assert_eq!(from_hsv(120., 1., 1.), 0x00ff00);
    }
    #[::core::prelude::v1::test]
    fn wall_ink_reads_on_the_card() {
        let d = Duo { main: 0x00a6a6, hi: 0xfff275 };
        assert_eq!(wall_ink(&d), [0x0c0d12, 0xfff275], "yellow shows on teal: the card's own highlight");
        let d = Duo { main: 0x00a6a6, hi: 0x00b8b8 };
        let [ink, hi] = wall_ink(&d);
        assert_eq!(ink, 0x0c0d12);
        assert_eq!(hi, 0xffffff, "a highlight too close to the card gives way to the other ink");
        let d = Duo { main: 0x101640, hi: 0x45f0d0 };
        assert_eq!(wall_ink(&d), [0xffffff, 0x45f0d0]);
    }
    #[::core::prelude::v1::test]
    fn any_hex() { assert_eq!(parse_any_hex("#abc"), Some(0xaabbcc)); assert_eq!(parse_any_hex("#6A35ff"), Some(0x6a35ff)); assert_eq!(parse_any_hex("#ab"), None); assert_eq!(parse_any_hex("#abcd"), None); assert_eq!(parse_any_hex("abc"), None); assert_eq!(parse_any_hex("#ggg"), None); }
    #[::core::prelude::v1::test]
    fn contrast_rule_matches_the_page() {
        // mega.js: contrast(a, b) = (max(lum)+.05)/(min(lum)+.05); the warning shows below 1.6
        assert!((contrast(0x000000, 0xffffff) - 21.).abs() < 0.01);
        assert!((contrast(0xffffff, 0xffffff) - 1.).abs() < 1e-6);
        assert!(contrast(0x00a6a6, 0x00b8b8) < 1.6, "too close");
        assert!(contrast(0x6a35ff, 0xff8c1a) >= 1.6);
        assert!(contrast(0x101640, 0x45f0d0) >= 1.6);
    }
    #[::core::prelude::v1::test]
    fn hex_check() { assert!(is_hex("#00a6A6")); assert!(!is_hex("00a6a6")); assert!(!is_hex("#00a6")); assert!(!is_hex("#00a6gg")); }
}
