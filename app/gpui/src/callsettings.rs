//! Call settings as the web shows them: a dialog in the middle of the window over a dark scrim (call-ui.js `settingsBody`,
//! call.css `#call-settings`): the microphone, speakers and camera as pick lists, the voice switches, push to talk, the camera
//! quality and how a screen is shared. The microphone's live level and the state of the system-wide keys are what this window
//! adds. Every choice goes to the engine, which remembers it.
use crate::app::Mega;
use crate::calls::audio::AudioDevice;
use crate::calls::camera::{CamProfile, CameraSource};
use crate::calls::engine::{self, Command};
use crate::calls::share::Profile;
use crate::look;
use crate::squircle as sq;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::rc::Rc;

type Pick = Rc<dyn Fn(&mut Mega, String, &mut Window, &mut Context<Mega>)>;

fn default_name(list: &[AudioDevice]) -> String {
    list.iter().find(|d| crate::calls::audio::is_default_entry(d)).map(|d| format!("System default ({})", d.name.trim_start_matches("default:").trim())).unwrap_or_else(|| "System default".into())
}

pub const SHARE_PROFILES: [(Profile, &str); 3] = [(Profile::Text, "Sharp text (up to 1440p, 30 fps)"), (Profile::Motion, "Smooth motion (1080p, 30 fps)"), (Profile::Motion60, "Fast motion (1080p, 60 fps)")];

impl Mega {
    /// The pick list the page draws as a `<select>`: the current choice in a box, the others in a list under it when it is open.
    fn select(&mut self, id: &'static str, current: String, options: Vec<(String, String, bool)>, pick: Pick, cx: &mut Context<Self>) -> Div {
        let open = self.call.dd == Some(id);
        let hits = self.call.hits.clone();
        let mut col = v_flex().w_full().gap(px(2.));
        col = col.child(div().id(id).relative().w_full().h(px(32.)).px(px(10.)).flex().items_center().justify_between().cursor_pointer().font_family(look::F_UI).text_size(px(13.))
            .child(sq::back(10., rgb(0x0b0a10))).child(sq::front(10., None, Some((1., look::line())), None)).child(hits.track(id))
            .child(div().relative().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(current))
            .child(div().relative().text_color(look::mute()).child(if open { "\u{25b4}" } else { "\u{25be}" }))
            .on_click(cx.listener(move |this, _, _, cx| { this.call.dd = if this.call.dd == Some(id) { None } else { Some(id) }; cx.notify(); })));
        if open {
            let mut list = v_flex().id(SharedString::from(format!("{id}-list"))).relative().w_full().max_h(px(190.)).overflow_y_scroll().p(px(4.)).gap(px(1.)).child(sq::back(10., rgb(0x1a1822)));
            for (i, (key, label, on)) in options.into_iter().enumerate() {
                let (pick, key2) = (pick.clone(), key.clone());
                list = list.child(div().id(SharedString::from(format!("{id}-{i}"))).relative().px(px(8.)).py(px(5.)).rounded(px(6.)).cursor_pointer().font_family(look::F_UI).text_size(px(13.))
                    .when(on, |d| d.bg(hsla(0., 0., 1., 0.14))).hover(|d| d.bg(hsla(0., 0., 1., 0.09))).child(label).child(hits.track(format!("{id}-{i}")))
                    .on_click(cx.listener(move |this, _, window, cx| { this.call.dd = None; pick(this, key2.clone(), window, cx); cx.notify(); })));
            }
            col = col.child(list);
        }
        col
    }

    fn setting(&self, label: &str, control: impl IntoElement) -> Div {
        h_flex().w_full().gap(px(14.)).items_start().child(div().flex_none().w(px(150.)).pt(px(8.)).font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child(label.to_lowercase())).child(div().flex_1().min_w_0().child(control))
    }

    pub fn settings_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.call.settings_open { return None; }
        let snap: engine::Snapshot = self.call.snap.clone();
        let hits = self.call.hits.clone();
        // microphone
        let mut mics: Vec<(String, String, bool)> = vec![(String::new(), default_name(&snap.devices.mic), snap.mic_id.is_none())];
        for d in &snap.devices.mic { mics.push((d.id.clone(), d.name.clone(), snap.mic_id.as_ref() == Some(&d.id))); }
        let mic_now = snap.mic_name.clone().unwrap_or_else(|| default_name(&snap.devices.mic));
        let mut outs: Vec<(String, String, bool)> = vec![(String::new(), default_name(&snap.devices.out), snap.out_id.is_none())];
        for d in &snap.devices.out { outs.push((d.id.clone(), d.name.clone(), snap.out_id.as_ref() == Some(&d.id))); }
        let out_now = snap.out_name.clone().unwrap_or_else(|| default_name(&snap.devices.out));
        let cam_on = !matches!(snap.camera.state.as_str(), "" | "off");
        let mut cams: Vec<(String, String, bool)> = vec![];
        for c in &snap.cameras { cams.push((c.id.clone(), c.name.clone(), snap.camera.id.as_deref() == Some(c.id.as_str()) || (snap.camera.id.is_none() && snap.cam_id.as_deref() == Some(c.id.as_str())))); }
        let cam_now = if cams.is_empty() { "No camera found on this computer".to_string() } else { cams.iter().find(|c| c.2).map(|c| c.1.clone()).unwrap_or_else(|| cams[0].1.clone()) };
        let cam_prof = snap.camera.profile.clone().unwrap_or_else(|| "720p".into());
        let share_now = self.call.share_profile;
        let entity = cx.entity().downgrade();
        let sw_hits = hits.clone();
        let sw = move |id: &'static str, on: bool, text: &'static str, which: u8| {
            let e = entity.clone();
            div().relative().self_start().child(sw_hits.track(id)).child(Switch::new(id).checked(on).label(text).on_click(move |v: &bool, _, cx| {
                let v = *v;
                let _ = e.update(cx, |this, cx| {
                    let s = this.call.snap.clone();
                    let mut p = crate::calls::audio::Processing { echo_cancellation: s.echo_cancellation, noise_suppression: s.noise_suppression, auto_gain_control: s.auto_gain_control };
                    match which { 0 => p.echo_cancellation = v, 1 => p = p.with_noise(v), _ => p.auto_gain_control = v }
                    if which == 3 { this.call_cmd(Command::SetPushToTalk(v)); } else { this.call_cmd(Command::SetProcessing(p)); }
                    cx.notify();
                });
            }))
        };
        let pick_mic: Pick = Rc::new(|this, id, _, _| this.call_cmd(Command::SetMicDevice { id }));
        let pick_out: Pick = Rc::new(|this, id, _, _| this.call_cmd(Command::SetOutDevice { id }));
        let pick_cam: Pick = Rc::new(move |this, id, _, _| this.call_cmd(if cam_on { Command::SwitchCamera(CameraSource::Device(Some(id))) } else { Command::StartCamera { source: CameraSource::Device(Some(id)), profile: None } }));
        let pick_prof: Pick = Rc::new(|this, k, _, _| this.call_cmd(Command::SetCameraProfile(if k == "1080p" { CamProfile::P1080 } else { CamProfile::P720 })));
        let pick_share: Pick = Rc::new(|this, k, _, _| {
            let p = SHARE_PROFILES.iter().find(|(p, _)| p.name() == k).map(|(p, _)| *p).unwrap_or(Profile::Text);
            this.call.share_profile = p;
            if this.call.snap.share.state != "idle" && !this.call.snap.share.state.is_empty() { this.call_cmd(Command::SetShareProfile(p)); }
        });
        let key_name = if self.call.ptt_key == "`" { "` (backtick)".to_string() } else { self.call.ptt_key.clone() };
        let shortcuts = if snap.global_shortcuts == "active" { "System-wide keys are on: they work with any window in front.".to_string() } else if snap.global_shortcuts_note.is_empty() { "The keys work while this window is in front.".to_string() } else { format!("Keys work while this window is in front. {}", snap.global_shortcuts_note) };
        let meter = {
            let pos = crate::calls::meter::meter_position(snap.mic_level);
            let hint = if snap.mic_system_muted == Some(true) { Some("Your microphone is switched off (muted) in your computer's sound settings. Unmute it there and the bar will move.")
                } else if snap.mic_quiet { Some("No sound is coming from this microphone. Check that it is the right one, plugged in, and not muted.") } else { None };
            v_flex().id("mic-meter").gap(px(4.)).child(div().id("mic-meter-bar").relative().h(px(8.)).w_full().rounded_full().bg(look::line()).child(div().id("mic-meter-fill").h_full().rounded_full().bg(if pos > 0.85 { rgb(look::DANGER).into() } else { look::paper() }).w(relative(pos))))
                .child(div().text_size(px(12.)).text_color(look::mute()).child(if snap.phase == "idle" || snap.phase.is_empty() { "Speak to test it. Nothing is played back." } else { "Speak to test it. This is what the others in the call receive." }))
                .children(hint.map(|h| div().id("mic-hint").text_size(px(12.)).text_color(rgb(look::DANGER)).child(h)))
        };
        let mic = self.select("cs-mic", mic_now, mics, pick_mic, cx);
        let out = self.select("cs-out", out_now, outs, pick_out, cx);
        let cam = self.select("cs-cam", cam_now, cams, pick_cam, cx);
        let prof = self.select("cs-camq", if cam_prof == "1080p" { "1080p".into() } else { "720p".into() }, vec![("720p".into(), "720p".into(), cam_prof != "1080p"), ("1080p".into(), "1080p".into(), cam_prof == "1080p")], pick_prof, cx);
        let share = self.select("cs-share", SHARE_PROFILES.iter().find(|(p, _)| *p == share_now).map(|x| x.1.to_string()).unwrap_or_default(),
            SHARE_PROFILES.iter().map(|(p, l)| (p.name().to_string(), l.to_string(), *p == share_now)).collect(), pick_share, cx);
        let body = v_flex().id("cs-body").w_full().gap(px(10.)).px(px(22.)).py(px(14.))
            .child(self.setting("Microphone", v_flex().gap(px(8.)).child(mic).child(meter)))
            .child(self.setting("Speakers", out))
            .child(self.setting("Camera", cam))
            .child(self.setting("Camera quality", prof))
            .child(self.setting("Share as", share))
            .child(self.setting("Voice", v_flex().gap(px(6.))
                .child(sw("cs-echo", snap.echo_cancellation, "Echo cancellation", 0)).child(sw("cs-noise", snap.noise_suppression, "Noise suppression", 1)).child(sw("cs-agc", snap.auto_gain_control, "Automatic gain", 2))
                .child(sw("cs-ptt", snap.push_to_talk, "Push to talk", 3))
                .child(div().text_size(px(12.)).text_color(look::mute()).child(format!("Hold the {key_name} key while this window is focused, or the microphone button on the call card. {shortcuts}")))));
        let panel = v_flex().id("call-settings-panel").relative().w(px(680.)).max_w_full().max_h(relative(0.88)).overflow_hidden().occlude()
            .child(sq::back(28., rgb(0x14131a))).child(sq::front(28., None, Some((1., look::line())), None)).on_click(|_, _, cx| cx.stop_propagation())
            .child(h_flex().relative().w_full().items_center().gap(px(10.)).px(px(22.)).py(px(12.)).border_b_1().border_color(look::line())
                .child(div().flex_1().font_family(look::F_SHARP).text_size(px(24.)).line_height(px(26.)).child("call settings"))
                .child(div().id("cs-close").relative().size(px(28.)).flex().items_center().justify_center().cursor_pointer().text_size(px(20.)).text_color(look::mute()).child("\u{d7}").child(hits.track("cs-close"))
                    .on_click(cx.listener(|this, _, _, cx| { this.set_settings_open(false); cx.notify(); }))))
            .child(div().id("cs-scroll").relative().flex_1().min_h_0().overflow_y_scroll().child(body));
        let _ = window;
        Some(div().id("call-settings").absolute().inset_0().flex().items_center().justify_center().p(px(24.)).bg(hsla(0., 0., 0., 0.6)).occlude()
            .on_click(cx.listener(|this, _, _, cx| { this.set_settings_open(false); cx.notify(); }))
            .child(panel).into_any_element())
    }
}
