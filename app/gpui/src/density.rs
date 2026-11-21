//! Chat density per room (mega-new.js section 3): how much chat fits on screen. Five stops for the Raster face (it is sharp
//! only on whole 12 px steps), nine for Bellefair (half steps, same ends). The middle stop sets nothing: the room looks as it
//! always did. Each stop is a size for ordinary messages, one for each length tier (short, shorter, shortest) and the space
//! between rows. "Same in every room" makes one value everyone's; otherwise each room remembers its own.
use crate::app::Mega;
use crate::look;
use crate::squircle as sq;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// What a stop sets, in the page's pixels: message, the three tiers, the row gap and the row padding.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dens { pub base: f32, pub tier: [f32; 3], pub gap: f32, pub pad: f32 }

/// Raster, from the page's CSS: stops -2, -1, 1, 2 (0 sets nothing).
const RASTER: [(i32, [f32; 6]); 4] = [(-2, [12., 12., 24., 36., 2., 0.]), (-1, [24., 24., 36., 48., 5., 1.]), (1, [36., 48., 72., 96., 14., 3.]), (2, [48., 72., 96., 120., 18., 4.])];
/// Bellefair, from -2 to 2 in half steps (the page's `FINE`).
const FINE: [Option<[f32; 6]>; 9] = [Some([12., 12., 24., 36., 2., 0.]), Some([15., 16., 28., 42., 3., 0.]), Some([18., 21., 34., 50., 4., 1.]), Some([21., 28., 40., 60., 6., 1.]), None,
    Some([28., 42., 56., 78., 10., 2.]), Some([33., 50., 68., 88., 12., 3.]), Some([40., 60., 82., 104., 15., 3.]), Some([48., 72., 96., 120., 18., 4.])];

pub fn step(raster: bool) -> f32 { if raster { 1. } else { 0.5 } }
pub fn clamp(v: f32, raster: bool) -> f32 {
    if !v.is_finite() { return 0.; }
    let s = step(raster);
    let r = (((v.clamp(-2., 2.) / s + 0.5).floor()) * s).clamp(-2., 2.);   // JS Math.round: halves go up
    if r == 0. { 0. } else { r }
}
pub fn name(n: f32) -> &'static str {
    match n { x if x <= -2. => "tightest", x if x <= -1. => "tighter", x if x < 0. => "a bit tighter", x if x == 0. => "as it was", x if x < 1. => "a bit larger", x if x < 2. => "larger", _ => "largest" }
}
/// The sizes for a stop, or None for the middle one. `k` is how much bigger the native text face sets than the page's (Bellefair 1.25).
pub fn of(level: f32, raster: bool) -> Option<Dens> {
    let n = clamp(level, raster);
    if n == 0. { return None; }
    let r = if raster { RASTER.iter().find(|(l, _)| *l as f32 == n).map(|(_, r)| *r)? } else { FINE[((n + 2.) * 2.) as usize]? };
    let k = if raster { 1. } else { 1.25 };
    Some(Dens { base: (r[0] * k).round(), tier: [(r[1] * k).round(), (r[2] * k).round(), (r[3] * k).round()], gap: r[4], pad: r[5] })
}

pub struct DensUi { pub open: bool, pub slider: Entity<SliderState>, pub _sub: Subscription, pub hits: crate::calldock::Hits }
impl DensUi {
    pub fn new(window: &mut Window, cx: &mut Context<Mega>) -> DensUi {
        let slider = cx.new(|_| SliderState::new().min(-2.).max(2.).step(0.5).default_value(0.));
        let sub = cx.subscribe_in(&slider, window, |this: &mut Mega, st, ev: &SliderEvent, window, cx| {
            if let SliderEvent::Change(v) = ev {
                let raster = this.prefs.chat_font == crate::prefs::ChatFont::Raster;
                let n = clamp(v.end(), raster);
                if (v.end() - n).abs() > 1e-4 { st.update(cx, |s, cx| s.set_value(n, window, cx)); }
                this.set_density(n, cx);
            }
        });
        DensUi { open: false, slider, _sub: sub, hits: Default::default() }
    }
}

impl Mega {
    pub fn density_now(&self) -> f32 { self.gid.as_deref().map(|g| self.prefs.dens_of(g)).unwrap_or(0.) }
    pub fn dens_params(&self) -> Option<Dens> { of(self.density_now(), self.prefs.chat_font == crate::prefs::ChatFont::Raster) }
    pub fn set_density(&mut self, n: f32, cx: &mut Context<Self>) {
        let Some(gid) = self.gid.clone() else { return };
        self.prefs.set_dens(&gid, n);
        self.prefs.save();
        self.list.remeasure(); self.reading.full.clear(); self.reading.scaled.clear();
        cx.notify();
    }
    pub fn dens_toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dens.open = !self.dens.open;
        if self.dens.open { let n = self.density_now(); self.dens.slider.update(cx, |s, cx| s.set_value(n, window, cx)); }
        cx.notify();
    }
    /// The small menu under the header: the slider, what the stop is called, and "same in every room".
    pub fn dens_popup(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.dens.open || self.level != crate::app::Level::Chat { return None; }
        let n = self.density_now();
        let all = self.prefs.dens_all;
        let hits = self.dens.hits.clone();
        let room = if all { "every room".to_string() } else { self.gid.as_deref().and_then(|g| self.channel(g)).map(|(_, ch)| ch.name.clone()).unwrap_or_else(|| "this room".into()) };
        Some(div().id("dens-pop").absolute().top(px(64.)).right(px(24.)).w(px(280.)).occlude().child(sq::back(16., rgb(0x14131a))).child(sq::front(16., None, Some((1., look::line())), None))
            .child(v_flex().relative().w_full().gap(px(10.)).p(px(16.)).font_family(look::F_UI).text_size(px(13.))
                .child(h_flex().justify_between().child(div().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child("density")).child(div().id("dens-for").font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child(room)))
                .child(div().relative().child(Slider::new(&self.dens.slider).horizontal()).child(hits.track("dens-slider")))
                .child(h_flex().justify_between().text_color(look::mute()).text_size(px(12.)).child("more on screen").child(div().id("dens-v").text_color(look::paper()).child(name(n))).child("larger"))
                .child(div().id("dens-all").relative().flex().gap(px(10.)).items_center().cursor_pointer().child(hits.track("dens-all"))
                    .child(div().size(px(16.)).flex_none().rounded(px(4.)).border_1().border_color(look::line()).when(all, |d| d.bg(look::paper())))
                    .child("same in every room")
                    .on_click(cx.listener(|this, _, _, cx| { let n = this.density_now(); this.prefs.set_dens_all(&this.gid.clone().unwrap_or_default(), !this.prefs.dens_all, n); this.prefs.save(); this.list.remeasure(); this.reading.full.clear(); cx.notify(); }))))
            .into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn the_middle_sets_nothing_and_the_ends_are_the_pages() {
        assert_eq!(of(0., true), None); assert_eq!(of(0., false), None);
        assert_eq!(of(-2., true), Some(Dens { base: 12., tier: [12., 24., 36.], gap: 2., pad: 0. }));
        assert_eq!(of(2., true).unwrap().tier, [72., 96., 120.]);
        assert_eq!(of(2., false).unwrap().base, 60.);
        assert_eq!(of(-0.5, false).unwrap().gap, 6.);
        assert_eq!(of(-0.5, true), None, "Raster has whole stops: -0.5 rounds to the middle");
    }
    #[::core::prelude::v1::test]
    fn values_snap_to_the_face_s_steps() {
        assert_eq!(clamp(0.6, false), 0.5); assert_eq!(clamp(0.6, true), 1.); assert_eq!(clamp(-0.5, true), 0.); assert_eq!(clamp(0.5, true), 1.); assert_eq!(clamp(9., true), 2.); assert_eq!(clamp(-9., false), -2.);
        assert_eq!(clamp(f32::NAN, true), 0.); assert!(clamp(-0.2, false).is_sign_positive());
        assert_eq!(name(0.), "as it was"); assert_eq!(name(-2.), "tightest"); assert_eq!(name(1.5), "larger");
    }
}
