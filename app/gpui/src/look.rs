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
    });
}

// ---- colours per room (mega.js: DEF, targetOf, dotOf, bestInk) ----
const DEF: [(u32, u32); 12] = [(0xff5a1f, 0xffe14d), (0x101640, 0x45f0d0), (0xf3c6d8, 0xc2124a), (0x0b3d2e, 0xd8ff3e), (0xe8e4d8, 0x1b33ff), (0x1a1a1a, 0xff2e63),
    (0x6a35ff, 0xffb3f0), (0x00a6a6, 0xfff275), (0xffd23f, 0x5a189a), (0x2d6a4f, 0xffb703), (0xe63946, 0xf1faee), (0x264653, 0xe9c46a)];

/// mega.js `H`: the hash that picks a room's default colours and pattern, so both UIs agree.
pub fn h(s: &str) -> u32 { s.chars().fold(7u32, |a, c| { let mut b = [0u16; 2]; a.wrapping_mul(31).wrapping_add(c.encode_utf16(&mut b)[0] as u32) }) }

fn parse_hex(s: &str) -> Option<u32> { u32::from_str_radix(s.strip_prefix('#')?, 16).ok() }

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
    pub fn high(&self) -> Hsla { rgb(self.hi).into() }
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
pub fn contrast(a: u32, b: u32) -> f32 { let (x, y) = (lum(a), lum(b)); (x.max(y) + 0.05) / (x.min(y) + 0.05) }

/// A person's colour (mega.js `faceColor`): hsl(H(name) % 360, 78%, 74%).
pub fn face(name: &str) -> Hsla { hsla((h(name) % 360) as f32 / 360., 0.78, 0.74, 1.) }
