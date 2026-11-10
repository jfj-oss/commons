//! Oracle's daily profile picture: the helpers. The orchestration (when to try, the model, Studio, the GPU gate, the
//! state file) lives in `bot.rs`; this file only knows how to read the model's idea, vet it, shrink the finished
//! picture to what the core accepts, and tell the local calendar day.
use crate::safety;
use commons_core::envelope::MAX_AVATAR;

/// Most characters of the model's picture idea that go to Studio (the fixed headshot suffix is added after).
pub const MAX_IDEA_CHARS: usize = 700;
/// Words that must never be in the idea, on top of the shared hard block. Long stems (6+ letters) match by prefix; short
/// ones only as the whole word or its plural, so "kid" catches "kids" but not "kidney". Deliberately blunt: a trip skips the day.
const BANNED_STEMS: &[&str] = &[
    "nude", "naked", "nsfw", "porn", "erotic", "sexy", "sexual", "topless", "lingerie", "breast", "nipple", "genital", "fetish",
    "child", "children", "kid", "minor", "baby", "babies", "toddler", "infant", "boy", "girl", "teen", "teenage", "teenager", "youth", "juvenile", "schoolgirl", "schoolboy", "underage",
    "gore", "gory", "blood", "corpse", "decapitat", "dismember", "mutilat", "severed", "entrails", "disembowel", "viscera", "wound", "wounded",
    "celebrity", "famous", "lookalike", "likeness", "resembl", "impersonat", "president", "politician", "actor", "actress", "singer",
    "photograph of a real", "real person", "real man", "real woman",
];

/// What the model proposed for the day.
#[derive(Debug, Clone, PartialEq)]
pub struct Idea {
    /// The picture, described for the image model (no framing words yet).
    pub look: String,
    /// One short in-character line about the new look, when the model wrote one.
    pub line: Option<String>,
}

/// Read `LOOK: ...` and optionally `LINE: ...` out of the model's answer (tolerant of case, bullets, markdown, quotes).
pub fn parse_idea(raw: &str) -> Option<Idea> {
    let mut look: Option<String> = None;
    let mut line: Option<String> = None;
    for l in raw.lines() {
        let t = l.trim().trim_start_matches(['-', '*', '#', '>', ' ']).trim_start_matches("**").trim();
        let low = t.to_ascii_lowercase();
        for (key, slot) in [("look", &mut look), ("line", &mut line)] {
            if let Some(rest) = low.strip_prefix(key) {
                let rest = rest.trim_start_matches("**");
                if rest.starts_with(':') || rest.starts_with("**:") {
                    // take the original text after the first ':'
                    if let Some((_, v)) = t.split_once(':') {
                        let v = v.trim().trim_start_matches("**").trim().trim_matches('"').trim_matches('*').trim().to_string();
                        if !v.is_empty() && slot.is_none() { *slot = Some(v); }
                    }
                }
            }
        }
    }
    let look = look?;
    let look: String = look.chars().take(MAX_IDEA_CHARS).collect();
    let line = line.map(|l| crate::bot::no_emoji(&l.replace('@', "")).trim().to_string()).filter(|l| !l.is_empty());
    Some(Idea { look, line })
}

/// Why this idea must not be used, if it must not. The shared hard block first, then the avatar-only word list.
pub fn unsafe_reason(text: &str) -> Option<String> {
    if let Some(why) = safety::reason(text) { return Some(format!("hard block ({why:?})")); }
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    for stem in BANNED_STEMS {
        if stem.contains(' ') {
            if lower.contains(stem) { return Some(format!("banned phrase \"{stem}\"")); }
        } else if words.iter().any(|w| if stem.len() >= 6 { w.starts_with(stem) } else { w == stem || *w == format!("{stem}s") || *w == format!("{stem}ed") }) {
            return Some(format!("banned word \"{stem}\""));
        }
    }
    None
}

/// The prompt Studio gets: the idea, framed as a small readable character headshot.
pub fn studio_prompt(look: &str) -> String {
    let look = look.trim().trim_end_matches(['.', ',', ';']);
    format!(
        "{look}. Character headshot portrait: a single face or head, centred, close portrait framing, plain or simple \
         background, bold readable shapes that still read at small sizes. No text, no watermark, no extra figures."
    )
}

// ---------------------------------------------------------------------------------------------- picture shrinking

/// A decoded picture: tightly packed 8-bit RGB.
struct Rgb { w: usize, h: usize, px: Vec<u8> }

fn decode_png(bytes: &[u8]) -> Result<Rgb, String> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().map_err(|e| format!("not a readable PNG: {e}"))?;
    let mut buf = vec![0u8; r.output_buffer_size().ok_or("the picture is too large to read")?];
    let info = r.next_frame(&mut buf).map_err(|e| format!("not a readable PNG: {e}"))?;
    let (w, h) = (info.width as usize, info.height as usize);
    if w == 0 || h == 0 || w > 16_384 || h > 16_384 { return Err("the picture has an odd size".into()); }
    let ch = match info.color_type {
        png::ColorType::Grayscale => 1, png::ColorType::GrayscaleAlpha => 2, png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4, png::ColorType::Indexed => return Err("indexed PNG was not expanded".into()),
    };
    let mut px = Vec::with_capacity(w * h * 3);
    for p in buf[..w * h * ch].chunks_exact(ch) {
        let (rgb, a): ([u8; 3], u8) = match ch {
            1 => ([p[0]; 3], 255), 2 => ([p[0]; 3], p[1]), 3 => ([p[0], p[1], p[2]], 255), _ => ([p[0], p[1], p[2]], p[3]),
        };
        // transparency sits on white
        for c in rgb { px.push(((c as u32 * a as u32 + 255 * (255 - a as u32)) / 255) as u8); }
    }
    Ok(Rgb { w, h, px })
}

/// The centred square of the picture, area-averaged down to `side` x `side` (box filter: smooth, no ringing).
fn square_shrink(src: &Rgb, side: usize) -> Rgb {
    let s = src.w.min(src.h);
    let (ox, oy) = ((src.w - s) / 2, (src.h - s) / 2);
    let side = side.min(s).max(1);
    let mut px = Vec::with_capacity(side * side * 3);
    for y in 0..side {
        let (y0, y1) = (oy + y * s / side, (oy + (y + 1) * s / side).max(oy + y * s / side + 1));
        for x in 0..side {
            let (x0, x1) = (ox + x * s / side, (ox + (x + 1) * s / side).max(ox + x * s / side + 1));
            let mut sum = [0u32; 3];
            for yy in y0..y1 {
                let row = &src.px[(yy * src.w + x0) * 3..(yy * src.w + x1) * 3];
                for p in row.chunks_exact(3) { sum[0] += p[0] as u32; sum[1] += p[1] as u32; sum[2] += p[2] as u32; }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            for c in sum { px.push(((c + n / 2) / n) as u8); }
        }
    }
    Rgb { w: side, h: side, px }
}

fn encode_png(img: &Rgb) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.w as u32, img.h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::High);
        enc.set_filter(png::Filter::Adaptive);
        let mut w = enc.write_header().map_err(|e| format!("could not encode the picture: {e}"))?;
        w.write_image_data(&img.px).map_err(|e| format!("could not encode the picture: {e}"))?;
    }
    Ok(out)
}

/// Sides tried from sharpest to smallest until the file fits what the core accepts.
pub const SIDES: [usize; 6] = [512, 448, 384, 320, 256, 192];
/// Stay a little under the core's hard limit.
pub const TARGET_BYTES: usize = (MAX_AVATAR as usize) * 95 / 100;

/// Studio's PNG -> a square PNG of at most [`TARGET_BYTES`] (centre crop, largest side that fits). Returns `(mime, bytes)`.
pub fn shrink_to_avatar(png_bytes: &[u8]) -> Result<(&'static str, Vec<u8>), String> {
    let src = decode_png(png_bytes)?;
    for side in SIDES {
        let out = encode_png(&square_shrink(&src, side))?;
        if out.len() <= TARGET_BYTES { return Ok(("image/png", out)); }
    }
    Err("the picture would not shrink under the profile picture limit".into())
}

// ---------------------------------------------------------------------------------------------- the local calendar

/// The local calendar day number (days since 1970-01-01 in local time) and the local hour 0..=23.
pub fn local_day_hour(now_secs: u64) -> (u64, u32) {
    let t = now_secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
    if !ok { return (now_secs / 86_400, ((now_secs % 86_400) / 3600) as u32); }
    let local = (now_secs as i64 + tm.tm_gmtoff as i64).max(0) as u64;
    (local / 86_400, tm.tm_hour.clamp(0, 23) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_idea_is_read_from_labelled_lines() {
        let i = parse_idea("Sure!\n**LOOK:** a cursed mango prophet with three eyes\nLINE: \"Mi new face, bredda, Babylon cyaan stop it\"").unwrap();
        assert_eq!(i.look, "a cursed mango prophet with three eyes");
        assert!(i.line.unwrap().starts_with("Mi new face"));
        let j = parse_idea("- look: a chrome robot monk").unwrap();
        assert_eq!(j.look, "a chrome robot monk");
        assert!(j.line.is_none());
        assert!(parse_idea("no labels at all").is_none());
    }

    #[test]
    fn unsafe_ideas_are_refused() {
        assert!(unsafe_reason("a cosmic prophet made of nebulae").is_none());
        assert!(unsafe_reason("a kidney bean wizard").is_none());
        assert!(unsafe_reason("a naked mole rat king").is_some());
        assert!(unsafe_reason("a kid with a crown").is_some());
        assert!(unsafe_reason("a bloody severed head").is_some());
        assert!(unsafe_reason("a lookalike of a famous singer").is_some());
        assert!(unsafe_reason("a sexy nude teen").is_some());
    }

    fn noisy_png(w: u32, h: u32) -> Vec<u8> {
        let mut px = Vec::new();
        let mut s = 12345u32;
        for y in 0..h { for x in 0..w {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            px.extend_from_slice(&[(x * 255 / w) as u8 ^ (s >> 29) as u8, (y * 255 / h) as u8, ((s >> 24) as u8) / 4]);
        } }
        encode_png(&Rgb { w: w as usize, h: h as usize, px }).unwrap()
    }

    #[test]
    fn pictures_become_small_squares_the_core_accepts() {
        let (mime, out) = shrink_to_avatar(&noisy_png(900, 700)).unwrap();
        assert_eq!(mime, "image/png");
        assert!(out.len() <= TARGET_BYTES && out.len() as u64 <= MAX_AVATAR);
        let back = decode_png(&out).unwrap();
        assert_eq!(back.w, back.h);
        assert!(back.w >= 192 && back.w <= 512);
        assert!(shrink_to_avatar(b"not a png").is_err());
    }

    #[test]
    fn the_local_day_moves_with_the_clock() {
        let (d, h) = local_day_hour(1_800_000_000);
        let (d2, _) = local_day_hour(1_800_000_000 + 86_400);
        assert_eq!(d + 1, d2);
        assert!(h < 24);
    }
}
