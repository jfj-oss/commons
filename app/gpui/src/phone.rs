//! The phone layout: one column, safe areas, 44 pt touch targets, larger text, taps and long presses instead of hover and right-click.
//!
//! It is a runtime flag, not a build one, so it can be tried on a desktop: it is on for the iPhone build, when MEGA_PHONE_PREVIEW is set
//! (the desktop then opens a 390 x 844 window, lib.rs), and whenever the window is narrower than `NARROW` points.
//! `sync` is called once at the top of the root's render; everything else reads `on()`.
use gpui_kit::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// Apple's minimum for a thing to tap.
pub const TAP: f32 = 44.;
/// A window narrower than this (logical points) is laid out as a phone.
pub const NARROW: f32 = 600.;
/// How long a finger must stay down, without moving, to count as a right-click.
pub const LONG_PRESS: std::time::Duration = std::time::Duration::from_millis(500);
/// How far a finger may wander and still be a tap or a long press.
pub const SLOP: f32 = 10.;

static ON: AtomicBool = AtomicBool::new(false);

pub fn preview() -> bool { std::env::var_os("MEGA_PHONE_PREVIEW").is_some_and(|v| v != "0") }
pub fn is_phone_width(width: f32, ios: bool, preview: bool) -> bool { ios || preview || width < NARROW }
pub fn on() -> bool { ON.load(Ordering::Relaxed) }
/// Sets the flag from the window; true when it changed.
pub fn sync(window: &Window) -> bool {
    let w = f32::from(window.viewport_size().width);
    let now = is_phone_width(w, cfg!(target_os = "ios"), preview());
    ON.swap(now, Ordering::Relaxed) != now
}

/// (top, bottom, left, right) the system keeps for the notch, the home bar and the rounded corners.
pub fn insets() -> (f32, f32, f32, f32) {
    if !on() { return (0., 0., 0., 0.); }
    #[cfg(target_os = "ios")]
    {
        // the on-screen keyboard covers the bottom of the page: lift it by the keyboard's height while it is up (the
        // keyboard's frame already includes the home bar, so the larger of the two, not their sum)
        let kb = gpui_mobile::keyboard_height();
        let (t, b, l, r) = gpui_mobile::safe_area_insets();
        let (t, b, l, r) = if t + b + l + r > 0. { (t, b, l, r) } else { (47., 34., 0., 0.) };
        return (t, b.max(kb), l, r);
    }
    // the desktop preview shows an iPhone 14's notch and home bar
    #[cfg(not(target_os = "ios"))]
    { (47., 34., 0., 0.) }
}

/// A finger that has moved this far from where it went down is dragging, not tapping.
pub fn far(a: Point<Pixels>, b: Point<Pixels>) -> bool { f32::from(a.x - b.x).hypot(f32::from(a.y - b.y)) > SLOP }

/// `a` on a desktop, `b` on a phone.
pub fn pick(a: f32, b: f32) -> f32 { if on() { b } else { a } }

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn narrow_windows_are_phones() {
        assert!(is_phone_width(390., false, false));
        assert!(!is_phone_width(1100., false, false));
        assert!(is_phone_width(1100., false, true));
        assert!(is_phone_width(1100., true, false));
        assert!(is_phone_width(NARROW - 1., false, false) && !is_phone_width(NARROW, false, false));
    }
}

// ---- long press: a finger held still is a right-click ----
use crate::app::Mega;
use std::cell::Cell;
thread_local! { static PRESS: Cell<Option<(Point<Pixels>, u64)>> = const { Cell::new(None) }; static GEN: Cell<u64> = const { Cell::new(0) }; }

impl Mega {
    /// A finger (the left button) went down: after LONG_PRESS without moving, send a right-click there and cancel the tap.
    pub fn press_begin(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        if !on() { return; }
        let g = GEN.with(|g| { g.set(g.get() + 1); g.get() });
        PRESS.with(|p| p.set(Some((at, g))));
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(LONG_PRESS).await;
            let go = this.update(cx, |_, _| {
                if PRESS.with(|p| p.get()).is_none_or(|(_, n)| n != g) { return false; }
                PRESS.with(|p| p.set(None));
                true
            }).unwrap_or(false);
            // outside the update of the view: the right-click's handlers update it themselves
            if go { let _ = cx.update(|window, cx| fire(at, window, cx)); }
        }).detach();
    }
    /// The finger moved: past the slop it is a drag (the list scrolls), not a press.
    pub fn press_move(&mut self, at: Point<Pixels>) {
        if let Some((t, _)) = PRESS.with(|p| p.get()) { if far(t, at) { PRESS.with(|p| p.set(None)); } }
    }
    pub fn press_end(&mut self) { PRESS.with(|p| p.set(None)); }
}

fn fire(at: Point<Pixels>, window: &mut Window, cx: &mut App) {
    let m = Modifiers::default();
    window.dispatch_event(PlatformInput::MouseDown(MouseDownEvent { button: MouseButton::Right, position: at, modifiers: m, click_count: 1, first_mouse: false }), cx);
    window.dispatch_event(PlatformInput::MouseUp(MouseUpEvent { button: MouseButton::Right, position: at, modifiers: m, click_count: 1 }), cx);
    // the finger is still down on whatever it pressed: end that press far away, so that lifting it later is not a tap or a click
    window.dispatch_event(PlatformInput::MouseUp(MouseUpEvent { button: MouseButton::Left, position: point(px(-10000.), px(-10000.)), modifiers: m, click_count: 1 }), cx);
}
