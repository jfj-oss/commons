//! Who is typing (app/ui/mega.js `typingDraw`, mega.css `#mega-typing`): the little antenna-squircle pixel mascot of each person who is
//! writing peeks over the top of the composer (bobbing, tapping its hands, blinking now and then), the words "X is typing" to its right
//! and a ripple of ASCII waves after them. A mascot slides up when its person starts and drops away when they stop. With the battery
//! saver on everything holds still: a still mascot, no waves, no slide.
use crate::ascii::{hole, Holes};
use crate::look::{self, Duo};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::component::h_flex;
use gpui_kit::*;
use std::cell::Cell;
use std::rc::Rc;

/// The mascot's body (`BOT`): `#` is body, `e` an eye (body coloured when the eyes are shut), rows 0..10; then the hands, two rows each (`HA`, `HB`).
const BOT: [&str; 10] = ["......#.....", "......#.....", "...######...", "..########..", ".##########.", ".##e####e##.", ".##e####e##.", ".##########.", ".##########.", "..########.."];
const HA: [&str; 2] = ["...##...##..", "...##......."];
const HB: [&str; 2] = ["...##...##..", "........##.."];
const EYE: u32 = 0x101116;
/// 12 cells in a 36 pixel box.
const CELL: f32 = 3.;
const SLIDE_IN: f64 = 0.38;
const SLIDE_OUT: f64 = 0.2;
/// The waves move one step each 66 ms (`setInterval(…, 66)`, phase + .42).
pub const WAVE_STEP: f64 = 0.066;
const GLYPHS: [char; 8] = [' ', '.', ':', '-', '=', ':', '.', ' '];

struct Who { name: String, t_in: f64, out: Option<f64> }

#[derive(Default)]
pub struct TypingUi {
    who: Vec<Who>,
    gid: Option<String>,
    /// The width of the waves' box, measured while painting (`ripW`): the first frame uses 60.
    wave_w: Rc<Cell<f32>>,
    /// The time the strip is drawn at: advanced only by the frame loop, so a redraw for some other reason while the window is hidden changes nothing.
    pub clock: f64,
}

/// What the words say (web `.mtwho`).
pub fn words(want: &[String]) -> String {
    let n = want.len();
    let shown: Vec<&str> = want.iter().take(3).map(|s| s.as_str()).collect();
    match n {
        0 => String::new(),
        1 => format!("{} is typing", want[0]),
        _ if n <= 3 => format!("{} and {} are typing", shown[..n - 1].join(", "), shown[n - 1]),
        _ => format!("{} and {} more are typing", shown.join(", "), n - 3),
    }
}

/// The waves' line of ASCII for a box this wide at this time.
pub fn waves(width: f32, t: f64) -> String {
    let n = (width / 7.4).ceil() as usize;
    let p = (t / WAVE_STEP).floor() * 0.42;
    (0..n).map(|i| GLYPHS[((((i as f64) * 0.34 - p).sin() + 1.) * 3.99).floor().clamp(0., 7.) as usize]).collect()
}

/// `cubic-bezier(x1, y1, x2, y2)` at time fraction t.
pub fn bezier(x1: f64, y1: f64, x2: f64, y2: f64, t: f64) -> f64 {
    let (mut lo, mut hi) = (0f64, 1f64);
    let at = |a: f64, b: f64, s: f64| 3. * (1. - s) * (1. - s) * s * a + 3. * (1. - s) * s * s * b + s * s * s;
    for _ in 0..24 { let m = (lo + hi) / 2.; if at(x1, x2, m) < t { lo = m } else { hi = m } }
    at(y1, y2, (lo + hi) / 2.)
}

/// How far down a mascot is (pixels): sliding up when it arrives, bobbing meanwhile, falling away when its person stops.
fn drop_of(w: &Who, now: f64, still: bool) -> f32 {
    if still { return 0.; }
    if let Some(o) = w.out { let k = ((now - o) / SLIDE_OUT).clamp(0., 1.); return (26. * bezier(0.42, 0., 1., 1., k)) as f32; }
    let a = now - w.t_in;
    if a < SLIDE_IN { return (30. * (1. - bezier(0.2, 1.5, 0.3, 1., (a / SLIDE_IN).max(0.)))) as f32; }
    0.
}
/// The bob (`mbob`: 1 px down, then 2 px up, in two jumps, back and forth every .56 s).
fn bob(age: f64) -> f32 { [1., -2., -2., 1.][((age.rem_euclid(1.12)) / 0.28) as usize & 3] }

fn mascot(name: String, age: f64, drop: f32, still: bool) -> impl IntoElement {
    let col = look::face(&name);
    canvas(|_, _, _| (), move |b, _, window, _| {
        let (ox, oy) = (f32::from(b.origin.x), f32::from(b.origin.y) + drop + if still { 0. } else { bob(age) });
        let blink = !still && age.rem_euclid(3.1) / 3.1 >= 0.93;
        let eye: Hsla = rgb(EYE).into();
        let mut cell = |x: usize, y: usize, c: Hsla, window: &mut Window| {
            let r = Bounds::new(point(px(ox + x as f32 * CELL), px(oy + y as f32 * CELL)), size(px(CELL * 1.02), px(CELL * 1.02)));
            window.paint_quad(fill(r, c));
        };
        for (y, row) in BOT.iter().enumerate() { for (x, ch) in row.chars().enumerate() {
            if ch == '#' || ch == 'e' { cell(x, y, col, window); }
        } }
        if !blink { for (y, row) in BOT.iter().enumerate() { for (x, ch) in row.chars().enumerate() { if ch == 'e' { cell(x, y, eye, window); } } } }
        // hands: the left pair and right pair tap in turn (both shown when still)
        let (a, bb) = if still { (true, true) } else { let on_a = age.rem_euclid(0.3) < 0.15; (on_a, !on_a) };
        for (on, rows) in [(a, &HA), (bb, &HB)] { if !on { continue; }
            for (y, row) in rows.iter().enumerate() { for (x, ch) in row.chars().enumerate() { if ch == '#' { cell(x, 10 + y, col, window); } } } }
    }).size(px(36.)).flex_none()
}

impl crate::app::Mega {
    /// Bring the mascots up to date with who the core says is typing in the open room. Called every frame of the room.
    pub fn typing_sync(&mut self, gid: &str) -> Vec<String> {
        let now = self.now_secs();
        let still = self.still();
        let u = &mut self.typing_ui;
        if u.who.is_empty() { u.clock = now; }
        if u.gid.as_deref() != Some(gid) { u.gid = Some(gid.to_string()); u.who.clear(); }
        let mut want: Vec<String> = vec![];
        for n in self.win.typing.iter() { if !want.contains(n) && want.len() < 6 { want.push(n.clone()); } }
        for w in u.who.iter_mut() { if w.out.is_none() && !want.contains(&w.name) { w.out = Some(now); } }
        u.who.retain(|w| w.out.map_or(true, |o| !still && now - o < SLIDE_OUT));
        for n in &want { if !u.who.iter().any(|w| &w.name == n && w.out.is_none()) { u.who.push(Who { name: n.clone(), t_in: if still { f64::NEG_INFINITY } else { now }, out: None }); } }
        want
    }
    /// Anything to draw at all (mascots on screen, or sliding off)?
    pub fn typing_on(&self) -> bool { !self.typing_ui.who.is_empty() }
    /// A slide is running, so a frame every tick rather than every wave step.
    pub fn typing_sliding(&self) -> bool {
        let now = self.now_secs();
        self.typing_ui.who.iter().any(|w| w.out.is_some() || now - w.t_in < SLIDE_IN)
    }

    /// The strip over the composer's top edge. It takes no room: it is zero tall and the row is drawn from just above the slip to a hair into it.
    pub fn typing_strip(&mut self, want: &[String], duo: Duo, holes: &Holes) -> Option<AnyElement> {
        if self.typing_ui.who.is_empty() { return None; }
        let (now, still) = (self.typing_ui.clock, self.still());
        let hi: Hsla = rgb(crate::backdrop::lift_over(duo.hi, crate::backdrop::BASE, 4.8)).into();
        let marks: Vec<AnyElement> = self.typing_ui.who.iter().map(|w| {
            let age = if still { 0. } else { now - w.t_in };
            div().relative().flex_none().size(px(36.)).child(mascot(w.name.clone(), age, drop_of(w, now, still), still)).child(hole(holes)).into_any_element()
        }).collect();
        let text = words(want);
        let ww = self.typing_ui.wave_w.clone();
        let wave = if still || want.is_empty() { String::new() } else { waves(if ww.get() > 1. { ww.get() } else { 60. }, now) };
        Some(div().relative().w_full().h(px(0.)).child(
            h_flex().absolute().top(px(-29.)).left(px(14.)).right(px(14.)).h(px(36.)).items_end().gap(px(8.)).overflow_hidden()
                .child(h_flex().flex_none().children(marks))
                .when(!text.is_empty(), |d| d.child(div().relative().flex_none().min_w_0().max_w(px(360.)).mb(px(12.)).ml(px(-2.)).mr(px(0.)).overflow_hidden().text_ellipsis().whitespace_nowrap()
                    .font_family(look::F_NAME).text_size(px(12.)).line_height(px(14.4)).text_color(look::paper()).child(text).child(hole(holes))))
                .child(div().relative().flex_1().min_w_0().h(px(24.)).pb(px(8.)).overflow_hidden().whitespace_nowrap().font_family(look::F_MONO).text_size(px(12.)).line_height(px(16.)).text_color(hi)
                    .child(canvas({ let ww = ww.clone(); move |b, _, _| ww.set(f32::from(b.size.width)) }, |_, _, _, _| {}).absolute().top_0().left_0().size_full())
                    .child(wave).child(hole(holes)))
        ).into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn the_words_say_who_is_typing() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(words(&[]), "");
        assert_eq!(words(&s(&["Bob"])), "Bob is typing");
        assert_eq!(words(&s(&["Bob", "Cy"])), "Bob and Cy are typing");
        assert_eq!(words(&s(&["A", "B", "C"])), "A, B and C are typing");
        assert_eq!(words(&s(&["A", "B", "C", "D", "E"])), "A, B, C and 2 more are typing");
    }
    #[::core::prelude::v1::test]
    fn the_waves_move_and_fill_the_box() {
        let (a, b) = (waves(100., 0.), waves(100., 0.2));
        assert_eq!(a.chars().count(), 14);
        assert_ne!(a, b, "the waves travel");
        assert_eq!(waves(100., 0.), waves(100., 0.04), "one step is 66 ms");
        assert!(a.chars().all(|c| " .:-=".contains(c)));
    }
    #[::core::prelude::v1::test]
    fn the_mascot_bobs_in_two_jumps_and_slides() {
        assert_eq!([bob(0.), bob(0.3), bob(0.6), bob(0.9), bob(1.2)], [1., -2., -2., 1., 1.]);
        let w = Who { name: "x".into(), t_in: 0., out: None };
        assert!((drop_of(&w, 0., false) - 30.).abs() < 0.01);
        assert!(drop_of(&w, 0.2, false) < 10.);
        assert_eq!(drop_of(&w, 1., false), 0.);
        let o = Who { name: "x".into(), t_in: 0., out: Some(1.) };
        assert!((drop_of(&o, 2., false) - 26.).abs() < 0.01);
        assert_eq!(drop_of(&w, 0., true), 0., "saver: no slide");
    }
}
