//! The camera for the call (CALLS-PLAN.md step 6). Everything about it goes through [`super::engine::Command`] and
//! [`super::engine::Snapshot`], like the screen share.
//!
//! # What the UI uses
//!
//! - **Which cameras there are**: `Command::ListCameras` fills `Snapshot.cameras` (`id`, `name`; on Linux the V4L2 capture nodes
//!   under `/dev/video*`, the id being the device path) and raises the change. The list is on demand, as the audio devices are.
//! - **Camera on**: `Command::StartCamera { source, profile }`. `source` is [`CameraSource::Device`] (`None` = the one used
//!   last time, else the first listed; `Some(id)` = that one) and `profile` is [`CamProfile`] (`720p` as in the web's default, or
//!   `1080p`; both are call.js `cameraProfiles`). Refused outside a call. While the device opens `camera.state` is `"starting"`; the
//!   first picture makes it `"live"`.
//! - **Camera off**: `Command::StopCamera`. As in the web client the track is **muted, not unpublished**: the same publication
//!   stays in the room with `muted`, its tile goes away for everyone, and turning the camera on again unmutes it (a new
//!   track only when the picture's size changed). The device is closed meanwhile (the camera light goes out).
//! - **Another camera**: `Command::SwitchCamera(CameraSource)`; the old one is closed first (a camera can be opened once), the
//!   same publication carries on when the new picture has the same size. The choice is remembered (`CallSettings.cam_id`).
//! - **Another profile**: `Command::SetCameraProfile(CamProfile)` (republishes with the new encodings; remembered).
//!
//! `Snapshot.camera` ([`CameraView`]): `state` (`off`, `starting`, `live`), `profile`, `source` (`device` or `test`), the device
//! `id` and `name`, the size and frame rate actually sent, `frames`, the publication's `sid`, `encrypted`, and `ended`
//! ([`CameraEnded`]: `seq`, `reason` = `system` (unplugged) / `error` (could not open: in use, no permission, no usable
//! format), `msg`), which stays until the next start or stop. A start that cannot happen is a passing `Snapshot.error` of kind
//! `camera`.
//!
//! **Your own picture**: `Snapshot.local_video` lists `"<your device id>/<sid>:Camera"` (and `:Screenshare` for a share) while that
//! track is live and not muted, in the same format as `subscribed`, and the picture of it reaches `video::Frames` under that sid
//! from the very buffer that goes to the encoder (no second capture, no re-encode).
//!
//! # How it works
//!
//! One task ([`CameraHandle`]) owns the capture thread (`camera_linux.rs` for a real device, or the synthetic source for tests),
//! the published track and the preview. It survives reconnects like the share task: a new session republishes from the
//! still-running capture. Everything published goes through the same rule as the microphone: it must come back encrypted
//! or it is unpublished, and the key index is pointed at before any frame is sent.
//!
//! The synthetic source ([`CameraSource::Test`]) is for the automated tests only (no real camera is ever opened by them): a
//! picture of the profile's size (1280x720 or 1920x1080), 30 fps, whose top half is reddish and bottom half bluish with a bar moving across, going through the same
//! fit, publish and preview path as a real device's pictures.
use super::capture::{self, Capture, Item, Limits};
use super::share::Sess;
use super::videoin::Preview;
use livekit::e2ee::EncryptionType;
use livekit::options::{TrackPublishOptions, VideoCodec, VideoEncoding, VideoPreset};
use livekit::prelude::*;
use livekit::webrtc::prelude::DegradationPreference;
use livekit::webrtc::video_frame::{I420Buffer, VideoBuffer, VideoFrame, VideoRotation};
use livekit::webrtc::video_source::{native::NativeVideoSource, RtcVideoSource, VideoResolution};
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// The two camera profiles of call.js `cameraProfiles`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum CamProfile {
    #[serde(rename = "720p")] P720,
    #[serde(rename = "1080p")] P1080,
}
#[derive(Clone, Copy, Debug)]
pub struct CamSpec { pub width: u32, pub height: u32, pub fps: f64, pub bitrate: u64, pub layers: [(u32, u32, u64, f64); 2] }
impl CamProfile {
    pub fn spec(self) -> CamSpec {
        match self {
            CamProfile::P720 => CamSpec { width: 1280, height: 720, fps: 30., bitrate: 2_500_000, layers: [(320, 180, 200_000, 15.), (640, 360, 800_000, 30.)] },
            CamProfile::P1080 => CamSpec { width: 1920, height: 1080, fps: 30., bitrate: 4_500_000, layers: [(640, 360, 800_000, 30.), (1280, 720, 2_500_000, 30.)] },
        }
    }
    pub fn name(self) -> &'static str { match self { CamProfile::P720 => "720p", CamProfile::P1080 => "1080p" } }
    pub fn parse(s: &str) -> Option<CamProfile> { match s { "720p" => Some(CamProfile::P720), "1080p" => Some(CamProfile::P1080), _ => None } }
    /// What `setCamera` in call.js passes: VP8, simulcast with two lower layers, balanced degradation.
    pub fn publish_options(self) -> TrackPublishOptions {
        let s = self.spec();
        TrackPublishOptions {
            source: TrackSource::Camera, video_codec: VideoCodec::VP8, simulcast: true,
            video_encoding: Some(VideoEncoding { max_bitrate: s.bitrate, max_framerate: s.fps }),
            simulcast_layers: Some(s.layers.iter().map(|l| VideoPreset::new(l.0, l.1, l.2, l.3)).collect()),
            degradation_preference: Some(DegradationPreference::Balanced), ..Default::default()
        }
    }
    pub fn limits(self) -> Limits { let s = self.spec(); Limits { max_w: s.width, max_h: s.height, fps: s.fps } }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CameraSource {
    /// A device by id (its path on Linux); `None` is the remembered one, else the first.
    Device(Option<String>),
    /// The synthetic source for tests.
    Test,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct CameraInfo { pub id: String, pub name: String }

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct CameraEnded { pub seq: u32, pub reason: String, pub msg: String }

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct CameraView {
    /// `"off"`, `"starting"` (the device is opening or its first picture is not here yet) or `"live"`.
    pub state: String,
    pub profile: Option<String>,
    /// `"device"` or `"test"`.
    pub source: Option<String>,
    pub id: Option<String>,
    pub name: Option<String>,
    /// The size of the picture being sent, and frames per second pushed in the last second.
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub frames: u64,
    pub sid: Option<String>,
    pub encrypted: bool,
    pub ended: Option<CameraEnded>,
}

/// What the camera task tells the engine.
pub enum CamReport {
    View(CameraView),
    Log(String),
    Fail(String),
}

pub enum CamCmd {
    Session(Option<Sess>),
    Start { source: CameraSource, profile: CamProfile, remembered: Option<String> },
    Stop,
    Switch(CameraSource),
    Profile(CamProfile),
    /// Tests: the device goes away as if it had been unplugged.
    TestEnd,
}

#[derive(Clone)]
pub struct CameraHandle { tx: UnboundedSender<CamCmd> }
impl CameraHandle { pub fn send(&self, c: CamCmd) { let _ = self.tx.send(c); } }

pub fn spawn(rt: &tokio::runtime::Handle, report: Arc<dyn Fn(CamReport) + Send + Sync>, preview: Option<Preview>) -> CameraHandle {
    let (tx, rx) = unbounded_channel();
    rt.spawn(Task::new(report, preview).run(rx));
    CameraHandle { tx }
}

struct VidPub { src: NativeVideoSource, publication: LocalTrackPublication, w: u32, h: u32 }

struct Task {
    report: Arc<dyn Fn(CamReport) + Send + Sync>,
    preview: Option<Preview>,
    sess: Option<Sess>,
    profile: CamProfile,
    cap: Option<Capture>,
    vid: Option<VidPub>,
    resize: Option<(u32, u32, Instant)>,
    view: CameraView,
    last_sent: Option<CameraView>,
    sec_start: Instant,
    sec_frames: u32,
    ended_seq: u32,
}

async fn next(c: Option<&mut Capture>) -> Option<Item> {
    match c { Some(c) => c.rx.recv().await, None => std::future::pending().await }
}
fn ts() -> i64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_micros() as i64).unwrap_or(0) }

impl Task {
    fn new(report: Arc<dyn Fn(CamReport) + Send + Sync>, preview: Option<Preview>) -> Task {
        Task { report, preview, sess: None, profile: CamProfile::P720, cap: None, vid: None, resize: None, view: CameraView { state: "off".into(), ..Default::default() }, last_sent: None, sec_start: Instant::now(), sec_frames: 0, ended_seq: 0 }
    }
    async fn run(mut self, mut rx: UnboundedReceiver<CamCmd>) {
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                c = rx.recv() => match c { Some(c) => self.cmd(c).await, None => break },
                it = next(self.cap.as_mut()) => self.on_item(it).await,
                _ = tick.tick() => self.on_tick(),
            }
            self.push_view();
        }
        self.stop(false).await;
    }
    fn log(&self, m: impl Into<String>) { (self.report)(CamReport::Log(m.into())); }
    fn fail(&self, m: impl Into<String>) { (self.report)(CamReport::Fail(m.into())); }
    fn push_view(&mut self) {
        if self.last_sent.as_ref() != Some(&self.view) { self.last_sent = Some(self.view.clone()); (self.report)(CamReport::View(self.view.clone())); }
    }
    fn ended(&mut self, reason: &str, msg: String) {
        self.ended_seq += 1;
        self.log(format!("camera ended ({reason}): {msg}"));
        self.view.ended = Some(CameraEnded { seq: self.ended_seq, reason: reason.into(), msg });
    }

    /// Opens the device (or the synthetic source) on a thread of its own.
    fn open(&mut self, source: &CameraSource, remembered: Option<&String>) {
        let limits = self.profile.limits();
        let (src, id, name) = match source {
            CameraSource::Test => ("test", Some("test".to_string()), Some("Test camera".to_string())),
            CameraSource::Device(want) => {
                let list = list();
                let pick = want.as_ref().or(remembered).and_then(|w| list.iter().find(|c| &c.id == w)).or(list.first());
                ("device", pick.map(|c| c.id.clone()).or(want.clone()), pick.map(|c| c.name.clone()))
            }
        };
        self.view.source = Some(src.into());
        self.view.id = id.clone();
        self.view.name = name;
        let cap = match source {
            CameraSource::Test => capture::start_with("mega-camera-test", limits, move |ctx| run_test(ctx)),
            CameraSource::Device(_) => {
                let (w, h, fps) = (self.profile.spec().width, self.profile.spec().height, self.profile.spec().fps);
                match id {
                    Some(id) => capture::start_with("mega-camera", limits, move |ctx| device_run(ctx, id, (w, h, fps as u32))),
                    None => capture::start_with("mega-camera", limits, |ctx| ctx.ended("There is no camera on this computer.", false)),
                }
            }
        };
        self.cap = Some(cap);
    }

    async fn cmd(&mut self, c: CamCmd) {
        match c {
            CamCmd::Session(s) => self.on_session(s).await,
            CamCmd::Start { source, profile, remembered } => {
                if self.sess.is_none() { self.fail("Join the call first."); return; }
                self.cap = None;
                self.view.ended = None;
                self.profile = profile;
                self.view.profile = Some(profile.name().into());
                self.view.state = "starting".into();
                self.open(&source, remembered.as_ref());
                self.log(format!("camera starting: {} profile, {} source", profile.name(), self.view.source.clone().unwrap_or_default()));
            }
            CamCmd::Switch(source) => {
                if self.view.state == "off" { self.fail("The camera is off."); return; }
                self.cap = None;   // a device opens once: the old one goes first
                std::thread::sleep(Duration::from_millis(60));
                self.view.state = "starting".into();
                self.view.ended = None;
                self.open(&source, None);
                self.log("switching camera");
            }
            CamCmd::Profile(p) => {
                self.profile = p;
                self.view.profile = Some(p.name().into());
                if self.cap.is_none() { return; }
                // the device is asked for the new size, and the track is published again with the new encodings
                let (src, id) = (self.view.source.clone(), self.view.id.clone());
                self.cap = None;
                self.unpublish_video().await;
                std::thread::sleep(Duration::from_millis(60));
                self.view.state = "starting".into();
                let source = if src.as_deref() == Some("test") { CameraSource::Test } else { CameraSource::Device(id) };
                self.open(&source, None);
                self.log(format!("camera profile: {}", p.name()));
            }
            CamCmd::Stop => { self.stop(true).await; self.view.ended = None; }
            CamCmd::TestEnd => { if let Some(c) = &self.cap { c.die(); } }
        }
    }

    async fn on_session(&mut self, s: Option<Sess>) {
        if let (Some(v), Some(p)) = (&self.vid, &self.preview) { p.forget(&v.publication.sid().to_string()); }
        self.vid = None;   // the old room is gone with its tracks
        self.resize = None;
        self.sess = s;
        if self.cap.is_some() { self.view.state = "starting".into(); }
        self.view.sid = None; self.view.encrypted = false;
    }

    async fn on_item(&mut self, it: Option<Item>) {
        match it {
            Some(Item::Frame(b)) => self.on_frame(b).await,
            Some(Item::Ended { msg, system }) => {
                self.stop(true).await;
                self.ended(if system { "system" } else { "error" }, msg.clone());
                if !system { self.fail(msg); }
            }
            None => { self.stop(true).await; self.ended("error", "The camera stopped.".into()); }
        }
    }

    async fn on_frame(&mut self, mut b: I420Buffer) {
        let (w, h) = (b.width(), b.height());
        self.sec_frames += 1;
        self.view.frames += 1;
        let Some(sess) = self.sess.clone() else { return };
        let same = self.vid.as_ref().is_some_and(|v| v.w == w && v.h == h);
        if !same {
            if self.vid.is_some() {
                match self.resize { Some((rw, rh, t)) if (rw, rh) == (w, h) => { if t.elapsed() < Duration::from_millis(700) { return; } } _ => { self.resize = Some((w, h, Instant::now())); return; } }
                self.unpublish_video().await;
            }
            if let Err(e) = self.publish_video(&sess, w, h).await {
                self.stop(true).await;
                self.ended("error", e.clone());
                self.fail(e);
                return;
            }
        }
        self.resize = None;
        if let Some(v) = &self.vid {
            if v.publication.is_muted() { v.publication.unmute(); }
            if let Some(p) = &self.preview { p.push(&v.publication.sid().to_string(), &mut b); }
            let mut f = VideoFrame::new(VideoRotation::VideoRotation0, b);
            f.timestamp_us = ts();
            v.src.capture_frame(&f);
        }
        self.view.width = w; self.view.height = h; self.view.state = "live".into();
    }

    async fn publish_video(&mut self, sess: &Sess, w: u32, h: u32) -> Result<(), String> {
        let src = NativeVideoSource::new(VideoResolution { width: w, height: h }, false);
        let track = LocalVideoTrack::create_video_track("camera", RtcVideoSource::Native(src.clone()));
        let p = sess.room.local_participant().publish_track(LocalTrack::Video(track), self.profile.publish_options()).await.map_err(|e| format!("The camera could not be published: {e}"))?;
        if p.encryption_type() == EncryptionType::None {
            let _ = sess.room.local_participant().unpublish_track(&p.sid()).await;
            return Err("The server did not mark your camera as encrypted, so it was stopped. Nothing was sent.".into());
        }
        sess.book.lock().point_senders(sess.room.e2ee_manager());
        self.view.sid = Some(p.sid().to_string());
        self.view.encrypted = true;
        self.log(format!("camera published: {w}x{h}, {} profile", self.profile.name()));
        self.vid = Some(VidPub { src, publication: p, w, h });
        Ok(())
    }

    async fn unpublish_video(&mut self) {
        if let Some(v) = self.vid.take() {
            if let Some(p) = &self.preview { p.forget(&v.publication.sid().to_string()); }
            if let Some(s) = self.sess.as_ref() { let _ = s.room.local_participant().unpublish_track(&v.publication.sid()).await; }
        }
        self.vid = None; self.resize = None; self.view.sid = None; self.view.encrypted = false;
    }

    /// Camera off: the device is closed and the track muted, not unpublished (as the web client does). `_user`: asked for.
    async fn stop(&mut self, _user: bool) {
        self.cap = None;
        if let Some(v) = &self.vid {
            if !v.publication.is_muted() { v.publication.mute(); }
            if let Some(p) = &self.preview { p.forget(&v.publication.sid().to_string()); }
        }
        self.resize = None;
        let (profile, ended) = (self.view.profile.clone(), self.view.ended.take());
        let sid = self.view.sid.clone(); let enc = self.view.encrypted;
        self.view = CameraView { state: "off".into(), profile, sid, encrypted: enc, ended, ..Default::default() };
    }

    fn on_tick(&mut self) {
        let e = self.sec_start.elapsed();
        if e >= Duration::from_secs(1) {
            self.view.fps = (self.sec_frames as f64 / e.as_secs_f64() * 10.).round() / 10.;
            self.sec_frames = 0; self.sec_start = Instant::now();
            if self.cap.is_none() { self.view.fps = 0.; }
        }
    }
}

// ------------------------------------------------------------------------------------------------ the devices

#[cfg(target_os = "linux")]
pub fn list() -> Vec<CameraInfo> { super::camera_linux::list() }
#[cfg(not(target_os = "linux"))]
pub fn list() -> Vec<CameraInfo> { vec![] }

#[cfg(target_os = "linux")]
fn device_run(ctx: capture::Ctx, id: String, want: (u32, u32, u32)) { super::camera_linux::run(ctx, id, want) }
#[cfg(not(target_os = "linux"))]
fn device_run(ctx: capture::Ctx, _id: String, _want: (u32, u32, u32)) { ctx.ended("Cameras are not built for this system yet.", false); }

// ------------------------------------------------------------------------------------------------ the test source

/// One synthetic picture: reddish top half, bluish bottom half, a white bar moving across. `n` is the frame number.
pub fn test_picture(w: u32, h: u32, n: u64) -> I420Buffer {
    let mut b = I420Buffer::new(w, h);
    let (sy, su, sv) = b.strides();
    let (y, u, v) = b.data_mut();
    let bar = ((n * 12) % w as u64) as usize;
    for row in 0..h as usize {
        let top = row < h as usize / 2;
        let base = if top { 150u8 } else { 90u8 };
        let line = &mut y[row * sy as usize..row * sy as usize + w as usize];
        line.fill(base);
        if row > h as usize / 3 && row < h as usize * 2 / 3 { for x in bar..(bar + 40).min(w as usize) { line[x] = 235; } }
    }
    for row in 0..(h / 2) as usize {
        let top = row < h as usize / 4;
        let (cu, cv) = if top { (90u8, 210u8) } else { (210u8, 90u8) };
        u[row * su as usize..row * su as usize + (w / 2) as usize].fill(cu);
        v[row * sv as usize..row * sv as usize + (w / 2) as usize].fill(cv);
        if row > h as usize / 6 && row < h as usize / 3 { for x in bar / 2..((bar + 40) / 2).min((w / 2) as usize) { u[row * su as usize + x] = 128; v[row * sv as usize + x] = 128; } }
    }
    b
}

fn run_test(ctx: capture::Ctx) {
    let t0 = Instant::now();
    let mut n: u64 = 0;
    loop {
        if ctx.stopped() { return; }
        if ctx.die.load(Ordering::SeqCst) { ctx.ended("the camera was unplugged (test)", true); return; }
        let l = ctx.limits();
        // the size the profile asks of a device: 1280x720 for 720p, 1920x1080 for 1080p
        let (w, h) = (l.max_w.max(2) & !1, l.max_h.max(2) & !1);
        ctx.frame(test_picture(w, h, n));
        n += 1;
        let due = t0 + Duration::from_secs_f64(n as f64 / l.fps.max(1.));
        let now = Instant::now();
        if due > now { std::thread::sleep(due - now); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profiles_match_call_js() {
        let s = CamProfile::P720.spec();
        assert_eq!((s.width, s.height, s.fps, s.bitrate), (1280, 720, 30., 2_500_000));
        assert_eq!(s.layers, [(320, 180, 200_000, 15.), (640, 360, 800_000, 30.)]);
        let s = CamProfile::P1080.spec();
        assert_eq!((s.width, s.height, s.bitrate), (1920, 1080, 4_500_000));
        assert_eq!(s.layers, [(640, 360, 800_000, 30.), (1280, 720, 2_500_000, 30.)]);
        for p in [CamProfile::P720, CamProfile::P1080] { assert_eq!(CamProfile::parse(p.name()), Some(p)); }
    }
    #[test]
    fn publish_options_are_the_webs() {
        let o = CamProfile::P720.publish_options();
        assert_eq!(o.source, TrackSource::Camera);
        assert_eq!(o.video_codec, VideoCodec::VP8);
        assert!(o.simulcast);
        assert_eq!(o.simulcast_layers.as_ref().map(|l| l.len()), Some(2));
        assert_eq!(o.degradation_preference, Some(DegradationPreference::Balanced));
        // the SDK lists the layers from the smallest: 320x180, 640x360, then the full picture
        let enc = livekit::options::compute_video_encodings(1280, 720, &o);
        assert_eq!(enc.len(), 3);
    }
    #[test]
    fn the_test_picture_has_two_tones() {
        let mut b = test_picture(1280, 720, 0);
        let (y, _, _) = b.data();
        let sy = b.strides().0 as usize;
        assert!(y[10 * sy + 5] > y[700 * sy + 5]);
        assert_eq!((b.width(), b.height()), (1280, 720));
        let _ = &mut b;
    }
}
