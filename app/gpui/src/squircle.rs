//! True squircles (superellipse corners), the shape the web draws with `corner-shape: squircle` (mega.css `.msq`).
//!
//! CSS `squircle` is `superellipse(2)`: each corner follows |x|^n + |y|^n = r^n with n = 2^2 = 4 inside its r x r box, instead of the
//! circle (n = 2) GPUI's quads draw. GPUI cannot clip to a path, so a squircle is made of three things:
//!   * `fill` / `stroke`: a filled or stroked path of the outline (backgrounds and borders),
//!   * `cutters`: the four slivers between the box's corners and the curve, painted in the colour behind the thing, on top of
//!     whatever the thing holds (so a card's field, weave and text are clipped to the squircle without a mask),
//!   * `contains`: the exact hit test, for clicks that land in a cut corner.
use gpui_kit::*;
use std::f32::consts::FRAC_PI_2;

/// The exponent of the corner curve (CSS squircle = superellipse(2) = exponent 2^2).
pub const N: f32 = 4.;
/// Points along one corner. The curve is steepest at its two ends, so they are sampled more densely (see `corner`).
const SEG: usize = 20;

/// The radius as used: never more than half the shorter side.
pub fn clamp_r(w: f32, h: f32, r: f32) -> f32 { r.min(w / 2.).min(h / 2.).max(0.) }

/// The curve of the top-left corner of a box of radius `r`, from the left edge (0, r) to the top edge (r, 0), as offsets inside the
/// r x r box: the points where (r-x)^n + (r-y)^n = r^n.
pub fn corner(r: f32) -> Vec<(f32, f32)> {
    let e = 2. / N;
    (0..=SEG).map(|i| {
        // denser at both ends: s runs 0..1, u = angle 0..pi/2 eased
        let s = i as f32 / SEG as f32;
        let u = FRAC_PI_2 * (1. - (std::f32::consts::PI * s).cos()) / 2.;
        // angle u from the top edge: x = r(1 - cos^e), y = r(1 - sin^e) walks from (0, r) to (r, 0) as u goes 0 -> pi/2
        (r * (1. - u.cos().max(0.).powf(e)), r * (1. - u.sin().max(0.).powf(e)))
    }).collect()
}

/// The whole outline of a box as a closed polygon, clockwise from the left end of the top edge.
pub fn outline(x: f32, y: f32, w: f32, h: f32, r: f32) -> Vec<(f32, f32)> {
    let r = clamp_r(w, h, r);
    if r <= 0.5 { return vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h)]; }
    let c = corner(r);
    let mut p = Vec::with_capacity(4 * c.len());
    // top-left corner: from the left edge up to the top edge
    for &(dx, dy) in &c { p.push((x + dx, y + dy)); }
    // top-right: mirror, walking from the top edge down to the right edge
    for &(dx, dy) in c.iter().rev() { p.push((x + w - dx, y + dy)); }
    // bottom-right: from the right edge to the bottom edge
    for &(dx, dy) in &c { p.push((x + w - dx, y + h - dy)); }
    // bottom-left: from the bottom edge up to the left edge
    for &(dx, dy) in c.iter().rev() { p.push((x + dx, y + h - dy)); }
    p
}

/// The four slivers outside the curve, each a polygon from the box's corner. `grow` pushes the box's corner outward so the
/// sliver also covers what the thing's own anti-aliased edge leaves.
pub fn slivers(x: f32, y: f32, w: f32, h: f32, r: f32, grow: f32) -> Vec<Vec<(f32, f32)>> {
    let r = clamp_r(w, h, r);
    if r <= 0.5 { return vec![]; }
    let c = corner(r);
    let g = grow;
    let mut out = Vec::with_capacity(4);
    // top-left: the corner point, then the curve from the top edge back to the left edge
    let mut tl = vec![(x - g, y + r), (x - g, y - g), (x + r, y - g)];
    for &(dx, dy) in c.iter().rev() { tl.push((x + dx, y + dy)); }
    out.push(tl);
    let mut tr = vec![(x + w - r, y - g), (x + w + g, y - g), (x + w + g, y + r)];
    for &(dx, dy) in &c { tr.push((x + w - dx, y + dy)); }
    out.push(tr);
    let mut br = vec![(x + w + g, y + h - r), (x + w + g, y + h + g), (x + w - r, y + h + g)];
    for &(dx, dy) in c.iter().rev() { br.push((x + w - dx, y + h - dy)); }
    out.push(br);
    let mut bl = vec![(x + r, y + h + g), (x - g, y + h + g), (x - g, y + h - r)];
    for &(dx, dy) in &c { bl.push((x + dx, y + h - dy)); }
    out.push(bl);
    out
}

/// Is the point inside the squircle (not in one of the cut corners)?
pub fn contains(px_: f32, py_: f32, x: f32, y: f32, w: f32, h: f32, r: f32) -> bool {
    if px_ < x || py_ < y || px_ > x + w || py_ > y + h { return false; }
    let r = clamp_r(w, h, r);
    if r <= 0.5 { return true; }
    // how far into the corner box, as a fraction of r (0 at the box edge where the curve starts, 1 at the box's corner)
    let a = ((x + r - px_) / r).max(0.);
    let b = ((y + r - py_) / r).max(0.);
    let a2 = ((px_ - (x + w - r)) / r).max(0.);
    let b2 = ((py_ - (y + h - r)) / r).max(0.);
    let (a, b) = (a.max(a2), b.max(b2));
    a == 0. || b == 0. || a.powf(N) + b.powf(N) <= 1.
}

/// Same, for a position and bounds from GPUI.
pub fn hit(pos: Point<Pixels>, b: Bounds<Pixels>, r: f32) -> bool {
    contains(f32::from(pos.x), f32::from(pos.y), f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height), r)
}

fn path(points: &[(f32, f32)], style: PathBuilder) -> Option<Path<Pixels>> {
    let mut b = style;
    let pts: Vec<Point<Pixels>> = points.iter().map(|&(x, y)| point(px(x), px(y))).collect();
    b.add_polygon(&pts, true);
    b.build().ok()
}

fn rect(b: Bounds<Pixels>) -> (f32, f32, f32, f32) { (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)) }

/// Paint the filled squircle.
pub fn fill(window: &mut Window, b: Bounds<Pixels>, r: f32, color: impl Into<Background>) {
    let (x, y, w, h) = rect(b);
    if let Some(p) = path(&outline(x, y, w, h, r), PathBuilder::fill()) { window.paint_path(p, color); }
}

/// Paint a border along the squircle, inside the box.
pub fn stroke(window: &mut Window, b: Bounds<Pixels>, r: f32, width: f32, color: Hsla) {
    let (x, y, w, h) = rect(b);
    let i = width / 2.;
    if let Some(p) = path(&outline(x + i, y + i, w - width, h - width, (r - i).max(0.)), PathBuilder::stroke(px(width))) { window.paint_path(p, color); }
}

/// Paint the four cut corners in the colour behind the thing.
pub fn cutters(window: &mut Window, b: Bounds<Pixels>, r: f32, behind: impl Into<Background> + Copy) {
    let (x, y, w, h) = rect(b);
    for s in slivers(x, y, w, h, r, 1.) { if let Some(p) = path(&s, PathBuilder::fill()) { window.paint_path(p, behind); } }
}

/// A layer that fills its (relative) parent and paints the squircle behind the parent's children.
pub fn back(r: f32, color: impl Into<Background> + Copy + 'static) -> impl IntoElement {
    canvas(|_, _, _| (), move |b, _, window, _| fill(window, b, r, color)).absolute().top_0().left_0().size_full()
}

/// Where a squircle layer last painted, so a click handler can test the exact shape (`clicked`).
pub type Spot = std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>;
pub fn spot() -> Spot { Spot::default() }
/// A click that landed in a cut corner is not a click on the thing.
pub fn clicked(spot: &Spot, ev: &ClickEvent, r: f32) -> bool { ev.mouse_position().is_none_or(|p| hit(p, spot.get(), r)) }

/// A layer that fills its (relative) parent and paints over the parent's children: the cut corners, then a border.
/// `spot` (from `spot()`) is told where it painted.
pub fn front(r: f32, behind: Option<Hsla>, border: Option<(f32, Hsla)>, spot: Option<Spot>) -> impl IntoElement {
    canvas(|_, _, _| (), move |b, _, window, _| {
        if let Some(s) = &spot { s.set(b); }
        if let Some(c) = behind { cutters(window, b, r, c); }
        if let Some((wd, c)) = border { stroke(window, b, r, wd, c); }
    }).absolute().top_0().left_0().size_full()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn the_curve_lies_on_the_superellipse() {
        let r = 60.;
        for (dx, dy) in corner(r) {
            let (a, b) = ((r - dx) / r, (r - dy) / r);
            assert!((a.powf(N) + b.powf(N) - 1.).abs() < 1e-3, "({dx}, {dy}) is off the curve: {}", a.powf(N) + b.powf(N));
        }
    }
    #[::core::prelude::v1::test]
    fn the_curve_runs_from_edge_to_edge() {
        let c = corner(40.);
        let (f, l) = (c[0], c[c.len() - 1]);
        assert!(f.0.abs() < 1e-3 && (f.1 - 40.).abs() < 1e-3, "starts on the left edge at the end of the arc: {f:?}");
        assert!((l.0 - 40.).abs() < 1e-3 && l.1.abs() < 1e-3, "ends on the top edge: {l:?}");
        // monotone: always moving right and up
        assert!(c.windows(2).all(|p| p[1].0 >= p[0].0 - 1e-4 && p[1].1 <= p[0].1 + 1e-4));
    }
    #[::core::prelude::v1::test]
    fn a_squircle_bulges_further_into_the_corner_than_a_circle() {
        // on the diagonal the circle is r(1 - 1/sqrt2) from the box corner; the superellipse (n=4) only r(1 - 1/4^(1/4)) = 0.293 r
        // vs 0.2929 r for the circle at n=2: the squircle cuts less of the corner, so its diagonal point is closer to the box corner
        let r = 100.;
        let mid = corner(r).into_iter().min_by(|a, b| ((a.0 - a.1).abs()).partial_cmp(&(b.0 - b.1).abs()).unwrap()).unwrap();
        let circle = r * (1. - std::f32::consts::FRAC_1_SQRT_2);
        assert!(mid.0 < circle, "the squircle's diagonal point {} lies nearer the corner than the circle's {circle}", mid.0);
        assert!((mid.0 - r * (1. - 0.5f32.powf(0.25))).abs() < 0.5);
    }
    #[::core::prelude::v1::test]
    fn the_outline_stays_inside_its_box_and_is_closed_in_order() {
        let p = outline(10., 20., 300., 150., 46.);
        assert!(p.iter().all(|&(x, y)| x >= 10. - 1e-3 && x <= 310. + 1e-3 && y >= 20. - 1e-3 && y <= 170. + 1e-3));
        assert_eq!(p.len(), 4 * (SEG + 1));
        // shoelace area is positive (clockwise on screen) and between the circle's and the full box's
        let a: f32 = (0..p.len()).map(|i| { let (x0, y0) = p[i]; let (x1, y1) = p[(i + 1) % p.len()]; x0 * y1 - x1 * y0 }).sum::<f32>() / 2.;
        let full = 300. * 150.;
        let circ = full - (4. - std::f32::consts::PI) * 46. * 46.;
        assert!(a > circ && a < full, "{a} between {circ} and {full}");
        // exact: the corner removes r^2 (1 - area under the curve), area of |x|^4+|y|^4<=1 quadrant = Gamma(1.25)^2/Gamma(1.5)
        let g = |q: f32| -> f32 { let mut s = 0.; let n = 200000; for i in 0..n { let t = (i as f32 + 0.5) / n as f32; s += (1. - t.powf(4.)).powf(0.25); } s / n as f32 + 0. * q };
        let area_q = g(0.);
        let cut = 46. * 46. * (1. - area_q);
        assert!((full - 4. * cut - a).abs() < 8., "area {a} vs {}", full - 4. * cut);
    }
    #[::core::prelude::v1::test]
    fn the_radius_is_never_more_than_half_the_short_side() {
        assert_eq!(clamp_r(100., 40., 46.), 20.);
        let p = outline(0., 0., 100., 40., 46.);
        assert!(p.iter().all(|&(_, y)| (-1e-3..=40. + 1e-3).contains(&y)));
    }
    #[::core::prelude::v1::test]
    fn the_slivers_and_the_squircle_tile_the_box() {
        let (w, h, r) = (200., 120., 50.);
        let area = |p: &[(f32, f32)]| -> f32 { (0..p.len()).map(|i| { let (x0, y0) = p[i]; let (x1, y1) = p[(i + 1) % p.len()]; x0 * y1 - x1 * y0 }).sum::<f32>().abs() / 2. };
        let inner = area(&outline(0., 0., w, h, r));
        let cuts: f32 = slivers(0., 0., w, h, r, 0.).iter().map(|s| area(s)).sum();
        assert!((inner + cuts - w * h).abs() < 2., "{inner} + {cuts} should be {}", w * h);
        // with no radius there is nothing to cut
        assert!(slivers(0., 0., w, h, 0., 1.).is_empty());
    }
    #[::core::prelude::v1::test]
    fn hit_testing_follows_the_curve() {
        let (x, y, w, h, r) = (0., 0., 200., 100., 40.);
        assert!(contains(100., 50., x, y, w, h, r));
        assert!(!contains(0.5, 0.5, x, y, w, h, r), "the very corner is cut");
        assert!(!contains(-1., 50., x, y, w, h, r));
        // just inside the corner on the diagonal: the squircle keeps a point the circle would cut (circle edge at 11.7 along the diagonal)
        let d = 40. * (1. - 0.5f32.powf(0.25));
        assert!(contains(d + 0.3, d + 0.3, x, y, w, h, r));
        assert!(!contains(d - 0.3, d - 0.3, x, y, w, h, r));
        let circle_d = 40. * (1. - std::f32::consts::FRAC_1_SQRT_2);
        assert!(contains(circle_d + 0.2, circle_d + 0.2, x, y, w, h, r));
        assert!(contains(199., 99., x, y, w, h, 0.), "no radius: the whole box");
        // all four corners behave the same way
        assert!(!contains(199.5, 0.5, x, y, w, h, r) && !contains(199.5, 99.5, x, y, w, h, r) && !contains(0.5, 99.5, x, y, w, h, r));
    }
}
