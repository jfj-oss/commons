//! The faces of the current look, loaded into GPUI's text system.
//!
//! The FK Raster trial files (the same ones the web UI ships in app/ui/fonts) name every cut of the Roman as one family
//! ("FK Raster Roman Compact Trial") and tell the cuts apart only by a style name (Rounded, Sharp), which a family +
//! weight + italic lookup cannot choose between. So in memory only, never on disk, each file's family-name records are
//! pointed at its own full name ("… Trial Rounded", "… Trial Sharp"): CSS does the same thing with @font-face names.
use std::borrow::Cow;

fn u16_at(b: &[u8], i: usize) -> Option<usize> { Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]) as usize) }
fn u32_at(b: &[u8], i: usize) -> Option<usize> { Some(u32::from_be_bytes([*b.get(i)?, *b.get(i + 1)?, *b.get(i + 2)?, *b.get(i + 3)?]) as usize) }

/// Family (name id 1) and typographic family (16) take the place and length of the full name (4) of the same platform.
fn family_from_full_name(font: &[u8]) -> Option<Vec<u8>> {
    let mut out = font.to_vec();
    let tables = u16_at(font, 4)?;
    let name = (0..tables).map(|i| 12 + i * 16).find(|&r| font.get(r..r + 4) == Some(b"name".as_slice())).and_then(|r| u32_at(font, r + 8))?;
    let count = u16_at(font, name + 2)?;
    let rec = |i: usize| name + 6 + i * 12;
    for i in 0..count {
        let id = u16_at(font, rec(i) + 6)?;
        if id != 1 && id != 16 { continue; }
        let full = (0..count).find(|&j| font[rec(j)..rec(j) + 6] == font[rec(i)..rec(i) + 6] && u16_at(font, rec(j) + 6) == Some(4))?;
        out.copy_within(rec(full) + 8..rec(full) + 12, rec(i) + 8);
    }
    Some(out)
}

pub fn all() -> Vec<Cow<'static, [u8]>> {
    let raster: [&'static [u8]; 4] = [
        include_bytes!("../../ui/fonts/FKRasterRomanCompactTrial-Rounded.otf"),
        include_bytes!("../../ui/fonts/FKRasterRomanCompactTrial-Sharp.otf"),
        include_bytes!("../../ui/fonts/FKRasterGroteskTrial-Rounded.otf"),
        include_bytes!("../../ui/fonts/FKRasterRomanCompactTrial-Blended.otf"),
    ];
    let mut v: Vec<Cow<'static, [u8]>> = raster.iter().map(|f| match family_from_full_name(f) { Some(p) => Cow::Owned(p), None => Cow::Borrowed(*f) }).collect();
    // Converted from the web UI's woff2 files (GPUI reads TrueType and OpenType, not woff2); see assets/fonts/README.md.
    v.push(Cow::Borrowed(include_bytes!("../assets/fonts/bellefair-regular.ttf")));
    v.push(Cow::Borrowed(include_bytes!("../assets/fonts/figtree-latin-wght-normal.ttf")));
    v.push(Cow::Borrowed(include_bytes!("../assets/fonts/ibm-plex-mono-latin-500-normal.ttf")));
    v
}
