//! The app's own window frame and title bar (client-side decorations).
//!
//! Layout, from the window's edge inward: the compositor-facing shadow margin and the 1 px light edge (both drawn by gpui_kit's
//! Root, `window_border`; the theme's `window_radius` and `window_border` colour style them, see `look::apply_theme`), then the dark
//! BAND (4 px) and, inside it, the content with ROUNDED inner corners. gpui clips to rectangles only, so the band is painted a second
//! time over the content as a rounded border whose hole has the inner radius. Maximized or tiled: no margin, edge, band or rounding (Root draws none).
//! The slim top row (`ROW`) holds the glossy traffic lights on the left and is the drag handle of the window. macOS keeps its native
//! lights (appears_transparent), the row only leaves room for them.
use crate::app::Mega;
use crate::look;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

pub const BAND: f32 = 4.;
/// Root's margin for the shadow (Linux) plus its 1 px edge.
pub const SHADOW: f32 = 20.;
pub const EDGE: f32 = 1.;
pub const R_IN: f32 = 20.;
pub const R_OUT: f32 = R_IN + BAND;
/// What the theme's `window_radius` is: the radius of the edge line, one more than the band's.
pub const R_EDGE: f32 = R_OUT + EDGE;
/// The slim top row below the band (the traffic lights' row).
pub const ROW: f32 = 24.;
pub const BALL: f32 = 14.;
pub const GAP: f32 = BALL * 0.4;
pub const LIGHTS_X: f32 = 12.;

pub fn band_colour(active: bool) -> Hsla { rgb(if active { 0x151515 } else { 0x121212 }).into() }
pub fn edge_colour(active: bool) -> Hsla { hsla(0., 0., 1., if active { 0.15 } else { 0.10 }) }

/// Sets the theme's edge colour (the 1 px line Root draws) for the window's focus state.
pub fn set_edge(active: bool, cx: &mut App) {
    use gpui_kit::component::Theme;
    // Root paints its surface (inside the edge line, at a radius that is one too round for the box) with the theme's background: left
    // opaque it covers pieces of the edge arc. mega's own frame covers the whole window, so the surface is transparent.
    let _ = active;
    Theme::update(cx, |t| { t.window_border = edge_colour(active); t.tokens.background = transparent_black().into(); });
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    /// The system draws the frame and bar (Windows, Linux where the compositor insisted).
    System,
    /// Client decorations, floating: shadow margin, edge, band, round inner corners.
    Framed,
    /// Client decorations, maximized or tiled: nothing around the content.
    Flush,
    /// macOS: native traffic lights over our row.
    Mac,
}

pub fn mode(window: &Window) -> Mode {
    if cfg!(target_os = "macos") { return Mode::Mac; }
    match window.window_decorations() {
        Decorations::Server => Mode::System,
        Decorations::Client { tiling } => if tiling.is_tiled() || window.is_maximized() || window.is_fullscreen() { Mode::Flush } else { Mode::Framed },
    }
}

/// Height of the top row (0 when the system draws its own bar).
pub fn row_h(m: Mode) -> f32 { if m == Mode::System { 0. } else { ROW } }
/// Per side, the space between the window's edge and the content (Root's margin and edge, and the band).
pub fn inset(m: Mode) -> f32 { if m == Mode::Framed { SHADOW + EDGE + BAND } else { 0. } }

/// The size mega lays itself out in: the window minus the frame and the top row. Use this, not `Window::viewport_size`.
pub fn viewport(window: &Window) -> Size<Pixels> {
    let m = mode(window);
    let s = window.viewport_size();
    let i = inset(m) * 2.;
    size(px((f32::from(s.width) - i).max(0.)), px((f32::from(s.height) - i - row_h(m)).max(0.)))
}

/// The size to ask for so that the content (below the row) is `w` x `h` (tests ask for a content size).
pub fn outer_for(w: f32, h: f32) -> (f32, f32) {
    if cfg!(target_os = "linux") { (w + 2. * (SHADOW + EDGE + BAND), h + 2. * (SHADOW + EDGE + BAND) + ROW) } else { (w, h + if cfg!(target_os = "macos") { ROW } else { 0. }) }
}

/// Any release in the window ends a press on a ball (the ball's own handler has already run: it is the child). True when one was pressed.
pub fn released() -> bool { PRESSED.with(|p| p.replace(None)).is_some() }

// ---- the lights ----
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Btn { Close, Min, Max }
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum St { Normal, Hover, Pressed, Backdrop }

static SVGS: [(&str, &[u8]); 12] = [
    ("close-normal", include_bytes!("../assets/titlebar/close-normal.svg")), ("close-hover", include_bytes!("../assets/titlebar/close-hover.svg")),
    ("close-pressed", include_bytes!("../assets/titlebar/close-pressed.svg")), ("close-backdrop", include_bytes!("../assets/titlebar/close-backdrop.svg")),
    ("minimize-normal", include_bytes!("../assets/titlebar/minimize-normal.svg")), ("minimize-hover", include_bytes!("../assets/titlebar/minimize-hover.svg")),
    ("minimize-pressed", include_bytes!("../assets/titlebar/minimize-pressed.svg")), ("minimize-backdrop", include_bytes!("../assets/titlebar/minimize-backdrop.svg")),
    ("maximize-normal", include_bytes!("../assets/titlebar/maximize-normal.svg")), ("maximize-hover", include_bytes!("../assets/titlebar/maximize-hover.svg")),
    ("maximize-pressed", include_bytes!("../assets/titlebar/maximize-pressed.svg")), ("maximize-backdrop", include_bytes!("../assets/titlebar/maximize-backdrop.svg")),
];

thread_local! { static PRESSED: std::cell::Cell<Option<Btn>> = std::cell::Cell::new(None); }
thread_local! { static BALL_DOWN: std::cell::Cell<bool> = std::cell::Cell::new(false); }
thread_local! { static CACHE: RefCell<HashMap<(&'static str, u32), Arc<RenderImage>>> = RefCell::new(HashMap::new()); }

/// One ball, rasterized at the device scale (so it is crisp at HiDPI), cached.
fn art(name: &'static str, dpr: f32) -> Option<Arc<RenderImage>> {
    let key = (name, (dpr * 100.).round() as u32);
    if let Some(i) = CACHE.with(|c| c.borrow().get(&key).cloned()) { return Some(i); }
    let bytes = SVGS.iter().find(|(n, _)| *n == name)?.1;
    let r = SvgRenderer::new(Arc::new(()));
    let px_ = (BALL * dpr).round() as i32;
    let parsed = r.parse_svg(bytes).ok()?;
    let img = r.render_parsed(&parsed, SvgSize::ExactSize(Size { width: DevicePixels(px_), height: DevicePixels(px_) })).ok()?;
    CACHE.with(|c| c.borrow_mut().insert(key, img.clone()));
    Some(img)
}

fn file(b: Btn, s: St) -> &'static str {
    match (b, s) {
        (Btn::Close, St::Normal) => "close-normal", (Btn::Close, St::Hover) => "close-hover", (Btn::Close, St::Pressed) => "close-pressed", (Btn::Close, St::Backdrop) => "close-backdrop",
        (Btn::Min, St::Normal) => "minimize-normal", (Btn::Min, St::Hover) => "minimize-hover", (Btn::Min, St::Pressed) => "minimize-pressed", (Btn::Min, St::Backdrop) => "minimize-backdrop",
        (Btn::Max, St::Normal) => "maximize-normal", (Btn::Max, St::Hover) => "maximize-hover", (Btn::Max, St::Pressed) => "maximize-pressed", (Btn::Max, St::Backdrop) => "maximize-backdrop",
    }
}

pub fn btn_id(b: Btn) -> &'static str { match b { Btn::Close => "tb-close", Btn::Min => "tb-min", Btn::Max => "tb-max" } }

fn ball(b: Btn, active: bool, dpr: f32, view: Entity<Mega>) -> impl IntoElement {
    let layer = |s: St| img(ImageSource::Render(art(file(b, s), dpr).expect("traffic light art"))).absolute().top_0().left_0().size(px(BALL)).object_fit(ObjectFit::Fill);
    let group = btn_id(b);
    if std::env::var_os("MEGA_TB_DEBUG").is_some() && PRESSED.with(|p| p.get()).is_some() { eprintln!("tb render pressed {:?} for {b:?}", PRESSED.with(|p| p.get())); }
    let pressed = PRESSED.with(|p| p.get()) == Some(b);
    let mut d = div().id(btn_id(b)).group(group).relative().flex_none().size(px(BALL)).cursor_pointer();
    d = if pressed { d.child(layer(St::Pressed)) } else {
        d.child(layer(if active { St::Normal } else { St::Backdrop })).child(layer(St::Hover).opacity(0.).group_hover(group, |s| s.opacity(1.)))
    };
    d
        // the press must not start a window move (the row's handler looks at this), and the action runs on the release over the ball (a click).
        // The pressed look is tracked here (the press is not stopped, so a later handler may still see it).
        .on_mouse_down(MouseButton::Left, { let view = view.clone(); move |ev, window, cx| { if std::env::var_os("MEGA_TB_DEBUG").is_some() { eprintln!("tb down {b:?} {:?}", ev.position); } BALL_DOWN.with(|d| d.set(true)); if std::env::var_os("MEGA_TB_ON_DOWN").is_some() { match b { Btn::Close => window.remove_window(), Btn::Min => window.minimize_window(), Btn::Max => window.zoom_window() } } PRESSED.with(|p| p.set(Some(b))); view.update(cx, |_, cx| cx.notify()); } })
        // a release over the ball that was pressed is the click (gpui's own click also fires after a release elsewhere)
        .on_mouse_up(MouseButton::Left, { let view = view.clone(); move |ev, window, cx| {
            if std::env::var_os("MEGA_TB_DEBUG").is_some() { eprintln!("tb up {b:?} {:?}", ev.position); }
            let was = PRESSED.with(|p| p.replace(None)) == Some(b);
            if std::env::var_os("MEGA_TB_DEBUG").is_some() { eprintln!("tb up was={was}"); }
            view.update(cx, |_, cx| cx.notify());
            if was { cx.stop_propagation(); match b { Btn::Close => window.remove_window(), Btn::Min => window.minimize_window(), Btn::Max => window.zoom_window() } }
        } })
}

/// The top row: the lights, and empty space that drags the window (double-click maximizes, right-click opens the window menu).
fn row(m: Mode, window: &Window, view: &Entity<Mega>) -> AnyElement {
    let active = window.is_window_active();
    let dpr = window.scale_factor();
    let client = m != Mode::System && m != Mode::Mac;
    let mut r = div().id("titlebar").flex().flex_row().items_center().flex_none().w_full().h(px(ROW)).pl(px(if m == Mode::Mac { 80. } else { LIGHTS_X })).gap(px(GAP))
        .window_control_area(WindowControlArea::Drag)
        .on_mouse_down(MouseButton::Left, move |ev, window, _| {
            if BALL_DOWN.with(|b| b.replace(false)) { return; }
            if ev.click_count >= 2 { if client { window.zoom_window() } else { window.titlebar_double_click() } } else { window.start_window_move() }
        })
        .on_mouse_down(MouseButton::Right, move |ev, window, _| { if client { window.show_window_menu(ev.position) } });
    if client { r = r.child(ball(Btn::Close, active, dpr, view.clone())).child(ball(Btn::Min, active, dpr, view.clone())).child(ball(Btn::Max, active, dpr, view.clone())); }
    r.into_any_element()
}

/// Wraps the page: the band, the rounded content and the top row. `page` is what used to fill the window.
pub fn frame(window: &Window, page: AnyElement, view: Entity<Mega>) -> AnyElement {
    let m = mode(window);
    let active = window.is_window_active();
    let framed = m == Mode::Framed;
    let band = band_colour(active);
    let inner = div().id("tb-inner").relative().flex().flex_col().flex_1().min_h_0().w_full().bg(rgb(look::VOID))
        .when(framed, |d| d.rounded(px(R_IN)))
        .when(m != Mode::System, |d| d.child(row(m, window, &view)))
        .child(page);
    let mut f = div().id("tb-frame").relative().flex().flex_col().size_full().bg(if framed { band } else { rgb(look::VOID).into() })
        .when(framed, |d| d.rounded(px(R_OUT)).p(px(BAND)))
        .child(inner);
    // the band again, on top of the content, as one rounded border: its outer radius is R_OUT and the border BAND wide, so the hole inside has
    // radius R_IN: the content's four corners are rounded without clipping anything (gpui clips to rectangles only). One quad, no path.
    if framed { f = f.child(div().absolute().top_0().left_0().size_full().rounded(px(R_OUT)).border(px(BAND)).border_color(band)); }
    f.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn geometry_adds_up() {
        assert_eq!(R_OUT, R_IN + BAND);
        assert_eq!(R_EDGE, R_OUT + EDGE);
        assert!((GAP - 5.6).abs() < 1e-4);
        let (w, h) = outer_for(1000., 700.);
        if cfg!(target_os = "linux") { assert_eq!((w, h), (1050., 774.)); }
    }
    #[::core::prelude::v1::test]
    fn art_is_all_there() {
        for (n, b) in SVGS.iter() { assert!(!b.is_empty(), "{n}"); }
        for b in [Btn::Close, Btn::Min, Btn::Max] { for s in [St::Normal, St::Hover, St::Pressed, St::Backdrop] { assert!(SVGS.iter().any(|(n, _)| *n == file(b, s))); } }
    }
}
