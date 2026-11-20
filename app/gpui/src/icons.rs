//! The call dock's icons, the same strokes as the web's own sprite (app/ui/call-ui.js `ci-*`, a 20x20 box, 1.6 wide, round ends).
//! GPUI has no SVG path parser of its own here, so the few shapes are written as small path strings (M L H V C A Z, absolute and
//! relative), flattened to polylines, and stroked in a canvas. Nothing is loaded from disk.
use gpui_kit::*;

/// The web's icon names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon { Mic, MicOff, Phones, Screen, Cam, Leave, Gear, Lock, Bell, Pop, Mini, Play, Pause, Next, Prev, Download, File, Clip, Close, Folder, Left, Right, Plus, Minus, Music }

/// One shape of an icon in the 20x20 box.
enum S { Path(&'static str), Rect(f32, f32, f32, f32, f32), Circle(f32, f32, f32) }

fn shapes(i: Icon) -> &'static [S] {
    use S::*;
    match i {
        Icon::Mic => &[Rect(7.5, 2.5, 5., 9., 2.5), Path("M4.5 9.5a5.5 5.5 0 0 0 11 0M10 15v2.5")],
        Icon::MicOff => &[Path("M3 3l14 14M7.5 7.5v1a2.5 2.5 0 0 0 4 2M12.5 8V5a2.5 2.5 0 0 0-5-.2M4.5 9.5a5.5 5.5 0 0 0 8.4 4.7M15.5 9.5c0 .8-.2 1.5-.5 2.2M10 15v2.5")],
        Icon::Phones => &[Path("M3.5 12V10a6.5 6.5 0 0 1 13 0v2"), Rect(2.5, 11.5, 3.5, 5., 1.2), Rect(14., 11.5, 3.5, 5., 1.2)],
        Icon::Screen => &[Rect(2.5, 3.5, 15., 10., 1.6), Path("M7 17h6M10 13.5V17")],
        Icon::Cam => &[Rect(2.5, 5.5, 10.5, 9., 1.8), Path("m13 9 4.5-2.5v7L13 11")],
        Icon::Leave => &[Path("M3.5 12.5c4-3.5 9-3.5 13 0l-1.2 2.2-2.8-.9-.3-2c-1.5-.5-3-.5-4.4 0l-.3 2-2.8.9z")],
        Icon::Gear => &[Path("M3 6h7.5M15.5 6H17M3 14h1.5M9.5 14H17"), Circle(13., 6., 2.3), Circle(7., 14., 2.3)],
        Icon::Lock => &[Rect(4.5, 9., 11., 8., 2.), Path("M7 9V6.5a3 3 0 0 1 6 0V9")],
        // a pair of eighth notes, in the sprite's stroke style
        Icon::Music => &[Path("M7.5 15V4.5l9-2V13"), Circle(5.5, 15., 2.), Circle(14.5, 13., 2.)],
        Icon::Bell => &[Path("M5 14V9a5 5 0 0 1 10 0v5l1.5 1.5h-13zM8.5 17.5h3")],
        // a window and the same window popped out of it (not in the web's sprite: drawn in its style)
        Icon::Pop => &[Rect(2.5, 6.5, 11., 11., 1.8), Path("M9 11l8-8M12 3h5v5")],
        Icon::Mini => &[Rect(2.5, 3.5, 15., 13., 1.8), Rect(9.5, 10., 6., 4.5, 1.)],
        Icon::Play => &[Path("M7 4.5l8 5.5-8 5.5z")],
        Icon::Pause => &[Path("M7 4.5v11M13 4.5v11")],
        Icon::Next => &[Path("M14 4.5v11M5 5l7 5-7 5z")],
        Icon::Prev => &[Path("M6 4.5v11M15 5l-7 5 7 5z")],
        Icon::Download => &[Path("M10 3v9.5M6 9l4 4 4-4M4 16.5h12")],
        Icon::File => &[Path("M5 2.5h6.5L15.5 6.5v11h-10.5zM11.5 2.5v4h4")],
        Icon::Clip => &[Path("M15.5 9.5l-5.8 5.8a3.2 3.2 0 0 1-4.5-4.5l6.2-6.2a2.1 2.1 0 0 1 3 3l-6.1 6.1a1 1 0 0 1-1.5-1.5l5.4-5.4")],
        Icon::Close => &[Path("M5 5l10 10M15 5L5 15")],
        Icon::Folder => &[Path("M2.5 5.5a1.5 1.5 0 0 1 1.5-1.5h3.5l1.8 2H16a1.5 1.5 0 0 1 1.5 1.5v7a1.5 1.5 0 0 1-1.5 1.5H4a1.5 1.5 0 0 1-1.5-1.5z")],
        Icon::Left => &[Path("M12.5 4l-6 6 6 6")],
        Icon::Right => &[Path("M7.5 4l6 6-6 6")],
        Icon::Plus => &[Path("M10 4v12M4 10h12")],
        Icon::Minus => &[Path("M4 10h12")],
    }
}

type Line = Vec<(f32, f32)>;

fn arc(out: &mut Line, from: (f32, f32), to: (f32, f32), rx: f32, ry: f32, large: bool, sweep: bool) {
    // endpoint to centre form (SVG implementation notes F.6.5), no x-axis rotation
    let (x1, y1) = from; let (x2, y2) = to;
    if rx == 0. || ry == 0. || (x1 == x2 && y1 == y2) { out.push(to); return; }
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    let (dx, dy) = ((x1 - x2) / 2., (y1 - y2) / 2.);
    let lam = (dx * dx) / (rx * rx) + (dy * dy) / (ry * ry);
    if lam > 1. { let s = lam.sqrt(); rx *= s; ry *= s; }
    let num = rx * rx * ry * ry - rx * rx * dy * dy - ry * ry * dx * dx;
    let den = rx * rx * dy * dy + ry * ry * dx * dx;
    let mut k = if den == 0. { 0. } else { (num / den).max(0.).sqrt() };
    if large == sweep { k = -k; }
    let (cxp, cyp) = (k * rx * dy / ry, -k * ry * dx / rx);
    let (cx, cy) = (cxp + (x1 + x2) / 2., cyp + (y1 + y2) / 2.);
    let ang = |ux: f32, uy: f32| uy.atan2(ux);
    let t1 = ang((dx - cxp) / rx, (dy - cyp) / ry);
    let mut dt = ang((-dx - cxp) / rx, (-dy - cyp) / ry) - t1;
    if sweep && dt < 0. { dt += std::f32::consts::TAU; }
    if !sweep && dt > 0. { dt -= std::f32::consts::TAU; }
    let n = ((dt.abs() * rx.max(ry)) * 2.).ceil().clamp(4., 40.) as usize;
    for i in 1..=n { let t = t1 + dt * i as f32 / n as f32; out.push((cx + rx * t.cos(), cy + ry * t.sin())); }
}

fn cubic(out: &mut Line, p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), p3: (f32, f32)) {
    for i in 1..=10 {
        let t = i as f32 / 10.; let u = 1. - t;
        out.push((u * u * u * p0.0 + 3. * u * u * t * p1.0 + 3. * u * t * t * p2.0 + t * t * t * p3.0, u * u * u * p0.1 + 3. * u * u * t * p1.1 + 3. * u * t * t * p2.1 + t * t * t * p3.1));
    }
}

/// An SVG path string as polylines (the subset above).
pub fn flatten(d: &str) -> Vec<Line> {
    // tokens: command letters and numbers (a number may start right after another: "-.5", ".5.5")
    let mut toks: Vec<String> = vec![];
    let b: Vec<char> = d.chars().collect();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_alphabetic() { toks.push(c.to_string()); i += 1; }
        else if c == '-' || c == '.' || c.is_ascii_digit() {
            let s = i; i += 1; let mut dot = c == '.';
            while i < b.len() && (b[i].is_ascii_digit() || (b[i] == '.' && !dot)) { if b[i] == '.' { dot = true; } i += 1; }
            toks.push(b[s..i].iter().collect());
        } else { i += 1; }
    }
    let mut lines: Vec<Line> = vec![]; let mut cur: Line = vec![]; let mut pos = (0f32, 0f32); let mut start = pos;
    let mut k = 0; let mut cmd = 'M';
    let num = |k: &mut usize| -> f32 { let v = toks[*k].parse().unwrap_or(0.); *k += 1; v };
    while k < toks.len() {
        if toks[k].chars().next().is_some_and(|c| c.is_ascii_alphabetic()) { cmd = toks[k].chars().next().unwrap(); k += 1; if cmd == 'Z' || cmd == 'z' { if !cur.is_empty() { cur.push(start); lines.push(std::mem::take(&mut cur)); } pos = start; continue; } }
        let rel = cmd.is_ascii_lowercase();
        let (ox, oy) = if rel { pos } else { (0., 0.) };
        match cmd.to_ascii_uppercase() {
            'M' => { let (x, y) = (num(&mut k) + ox, num(&mut k) + oy); if !cur.is_empty() { lines.push(std::mem::take(&mut cur)); } pos = (x, y); start = pos; cur.push(pos); cmd = if rel { 'l' } else { 'L' }; }
            'L' => { let (x, y) = (num(&mut k) + ox, num(&mut k) + oy); pos = (x, y); cur.push(pos); }
            'H' => { let x = num(&mut k) + ox; pos.0 = x; cur.push(pos); }
            'V' => { let y = num(&mut k) + oy; pos.1 = y; cur.push(pos); }
            'C' => { let p1 = (num(&mut k) + ox, num(&mut k) + oy); let p2 = (num(&mut k) + ox, num(&mut k) + oy); let p3 = (num(&mut k) + ox, num(&mut k) + oy); cubic(&mut cur, pos, p1, p2, p3); pos = p3; }
            'A' => { let (rx, ry) = (num(&mut k), num(&mut k)); let _rot = num(&mut k); let (la, sw) = (num(&mut k) != 0., num(&mut k) != 0.); let to = (num(&mut k) + ox, num(&mut k) + oy); arc(&mut cur, pos, to, rx, ry, la, sw); pos = to; }
            _ => { k += 1; }
        }
    }
    if !cur.is_empty() { lines.push(cur); }
    lines
}

fn rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Line {
    let mut out = vec![(x + r, y), (x + w - r, y)];
    arc(&mut out, (x + w - r, y), (x + w, y + r), r, r, false, true); out.push((x + w, y + h - r));
    arc(&mut out, (x + w, y + h - r), (x + w - r, y + h), r, r, false, true); out.push((x + r, y + h));
    arc(&mut out, (x + r, y + h), (x, y + h - r), r, r, false, true); out.push((x, y + r));
    arc(&mut out, (x, y + r), (x + r, y), r, r, false, true);
    out
}
fn circle(cx: f32, cy: f32, r: f32) -> Line { (0..=32).map(|i| { let t = std::f32::consts::TAU * i as f32 / 32.; (cx + r * t.cos(), cy + r * t.sin()) }).collect() }

/// All the polylines of an icon in its 20x20 box.
pub fn lines(i: Icon) -> Vec<Line> {
    let mut v = vec![];
    for s in shapes(i) { match s { S::Path(d) => v.extend(flatten(d)), S::Rect(x, y, w, h, r) => v.push(rect(*x, *y, *w, *h, *r)), S::Circle(x, y, r) => v.push(circle(*x, *y, *r)) } }
    v
}

/// The icon drawn at `size` px in `color` (an element of exactly that size; the 20x20 box scales to it, the stroke with it).
pub fn icon(i: Icon, size: f32, color: Hsla) -> impl IntoElement {
    let ls = lines(i);
    canvas(|_, _, _| (), move |b, _, window, _| {
        let k = size / 20.;
        let (ox, oy) = (f32::from(b.origin.x), f32::from(b.origin.y));
        let w = 1.6 * k;
        for l in &ls {
            if l.len() < 2 { continue; }
            let mut pb = PathBuilder::stroke(px(w));
            pb.add_polygon(&l.iter().map(|&(x, y)| point(px(ox + x * k), px(oy + y * k))).collect::<Vec<_>>(), false);
            if let Ok(p) = pb.build() { window.paint_path(p, color); }
            // round ends: a dot of half the stroke at each end
            for &(x, y) in [l[0], l[l.len() - 1]].iter() {
                let mut pb = PathBuilder::fill();
                let pts: Vec<Point<Pixels>> = (0..12).map(|j| { let t = std::f32::consts::TAU * j as f32 / 12.; point(px(ox + x * k + w / 2. * t.cos()), px(oy + y * k + w / 2. * t.sin())) }).collect();
                pb.add_polygon(&pts, true);
                if let Ok(p) = pb.build() { window.paint_path(p, color); }
            }
        }
    }).size(px(size))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn every_icon_flattens_inside_its_box() {
        for i in [Icon::Mic, Icon::MicOff, Icon::Phones, Icon::Screen, Icon::Cam, Icon::Leave, Icon::Gear, Icon::Lock, Icon::Bell, Icon::Pop, Icon::Mini, Icon::Play, Icon::Pause, Icon::Download, Icon::File, Icon::Clip, Icon::Close, Icon::Folder, Icon::Left, Icon::Right, Icon::Plus, Icon::Minus, Icon::Next, Icon::Prev] {
            let ls = lines(i);
            assert!(!ls.is_empty(), "{i:?}");
            for l in ls { for (x, y) in l { assert!(x.is_finite() && y.is_finite() && (-0.5..=20.5).contains(&x) && (-0.5..=20.5).contains(&y), "{i:?} {x},{y}"); } }
        }
    }
    #[::core::prelude::v1::test]
    fn the_arc_of_the_headphone_band_peaks_at_the_top() {
        let l = flatten("M3.5 12V10a6.5 6.5 0 0 1 13 0v2");
        let top = l.iter().flatten().map(|p| p.1).fold(f32::MAX, f32::min);
        assert!((top - 3.5).abs() < 0.2, "{top}");
    }
}
