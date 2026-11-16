//! Screen share and one app's sound for the call (CALLS-PLAN.md section 6 and the audio half of section 7).
//!
//! # What the UI uses
//!
//! All of it goes through [`super::engine::Command`] and [`super::engine::Snapshot`]:
//!
//! - **Start** a share: `Command::StartShare { profile, source, audio }`. `source` is [`ShareSource::Portal`] (the
//!   system's own picker opens; the person picks a screen or a window in it) and `audio` is an optional app stream id
//!   from `Snapshot.share_apps` whose sound goes with it. While the picker is open `share.state` is `"starting"`; the
//!   first picture makes it `"live"`. Cancelling the picker ends it with `share.ended` (reason `"error"`, a message).
//! - **Stop**: `Command::StopShare` (stops the picture and, if it was started with one, its sound).
//! - **Switch what is shared** mid-call: `Command::SwitchShareSource(ShareSource::Portal)` opens the picker again; the
//!   old share keeps running until a new screen is picked (cancelling keeps the old one and says so in the events), then
//!   the picture changes over. A picture of a different size is republished (a new track), the same size is not.
//! - **Change the profile** mid-call: `Command::SetShareProfile(Profile)` (`text`, `motion`, `motion60`). It republishes
//!   with the new encodings; the capture, and so the portal session, is not touched.
//! - **Sound of one app**: `Command::ListShareApps` fills `Snapshot.share_apps` (id, application name, pid, media name,
//!   whether it is a browser); `Command::StartShareAudio { stream }` and `StopShareAudio`. It can run alone or with a
//!   share (then it stops with it). Published as the web's PC audio is: track name `music-audio`, source
//!   `ScreenshareAudio`, stereo, 192 kbit/s, no DTX; so web clients treat it the same way (their music backdrop listens
//!   to it, and `call.js` lists it as a screen-share-audio track).
//!
//! `Snapshot.share` ([`ShareView`]) says what is going on: `state` (`idle`, `starting`, `live`), the profile, the source,
//! the size and frame rate actually sent, the track's sid, whether it is encrypted, and `audio` ([`ShareAudioView`]) with
//! the app, a 0..1 level for a meter and `silent` (nothing heard for 5 s, the web's "no sound yet" note).
//! **Events**: `share.ended` is `Some(ShareEnded { seq, what, reason, msg })` when a share or its sound ended without
//! the person asking: `reason` is `"system"` (ended from the system's stop-sharing control, or the portal session
//! closed), `"app-gone"` (the app closed its stream) or `"error"` (it could not start or the picker was cancelled).
//! `seq` rises each time, so a UI can tell a new ending from one it showed. It stays until the next start or stop. The
//! same facts are lines in `Snapshot.events`, and a start that cannot happen (not in a call, the app is gone) is a
//! passing `Snapshot.error` of kind `screen`.
//!
//! # How it works
//!
//! One task ([`ShareHandle`]) owns the capture (`capture.rs`), the published tracks and the audio tap. It survives
//! reconnects: when the engine builds a new session it hands it over ([`ShareCmd::Session`]) and the tracks are
//! published again from the still-running capture. The picture track is created when the first frame says how big the
//! picture is (the portal tells nothing before that); frames are fitted inside the profile's ceiling keeping their shape.
//! Everything published goes through the same rule as the microphone: it must come back encrypted or it is unpublished,
//! and the key index is pointed at before any frame is sent.
use super::appaudio::{self, AppStream};
use super::capture::{self, Capture, Item, Limits, ShareSource};
use super::keys::KeyBook;
use livekit::e2ee::EncryptionType;
use livekit::options::{AudioEncoding, TrackPublishOptions, VideoCodec, VideoEncoding, VideoPreset};
use livekit::prelude::*;
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::{native::NativeAudioSource, AudioSourceOptions, RtcAudioSource};
use livekit::webrtc::prelude::DegradationPreference;
use livekit::webrtc::video_frame::{VideoBuffer, VideoFrame, VideoRotation};
use livekit::webrtc::video_source::{native::NativeVideoSource, RtcVideoSource, VideoResolution};
use parking_lot::Mutex;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

// ------------------------------------------------------------------------------------------------ profiles

/// The three share profiles of call.js `shareProfiles`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    /// "Sharp text (up to 1440p, 30 fps)": contentHint detail, maintain-resolution.
    Text,
    /// "Smooth motion (1080p, 30 fps)".
    Motion,
    /// "Fast motion (1080p, 60 fps)".
    Motion60,
}

/// Numbers of one profile, as in call.js.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    pub width: u32,
    pub height: u32,
    pub bitrate: u64,
    pub fps: f64,
    /// The one fallback layer: width, height, bit rate, frame rate.
    pub layer: (u32, u32, u64, f64),
    /// `contentHint: 'detail'` (true) or `'motion'` (false).
    pub screencast: bool,
    pub keep: DegradationPreference,
}

impl Profile {
    pub fn spec(self) -> Spec {
        match self {
            Profile::Text => Spec { width: 2560, height: 1440, bitrate: 6_000_000, fps: 30., layer: (960, 540, 350_000, 5.), screencast: true, keep: DegradationPreference::MaintainResolution },
            Profile::Motion => Spec { width: 1920, height: 1080, bitrate: 6_000_000, fps: 30., layer: (960, 540, 600_000, 15.), screencast: false, keep: DegradationPreference::MaintainFramerate },
            Profile::Motion60 => Spec { width: 1920, height: 1080, bitrate: 9_000_000, fps: 60., layer: (960, 540, 600_000, 15.), screencast: false, keep: DegradationPreference::MaintainFramerate },
        }
    }
    pub fn name(self) -> &'static str { match self { Profile::Text => "text", Profile::Motion => "motion", Profile::Motion60 => "motion60" } }
    pub fn parse(s: &str) -> Option<Profile> { match s { "text" => Some(Profile::Text), "motion" => Some(Profile::Motion), "motion60" => Some(Profile::Motion60), _ => None } }

    /// The full picture plus ONE 540p layer, VP8, tagged as a screen share.
    pub fn publish_options(self) -> TrackPublishOptions {
        let s = self.spec();
        TrackPublishOptions {
            source: TrackSource::Screenshare, video_codec: VideoCodec::VP8, simulcast: true,
            video_encoding: Some(VideoEncoding { max_bitrate: s.bitrate, max_framerate: s.fps }),
            simulcast_layers: Some(vec![VideoPreset::new(s.layer.0, s.layer.1, s.layer.2, s.layer.3)]),
            degradation_preference: Some(s.keep), ..Default::default()
        }
    }
}

/// How the shared app's sound is published: what call-pcaudio.js publishes for the PC audio route.
pub const AUDIO_TRACK_NAME: &str = "music-audio";
pub fn audio_publish_options() -> TrackPublishOptions {
    TrackPublishOptions { source: TrackSource::ScreenshareAudio, audio_encoding: Some(AudioEncoding { max_bitrate: 192_000 }), dtx: false, red: true, ..Default::default() }
}

// ------------------------------------------------------------------------------------------------ what the UI sees

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct ShareAudioView {
    pub stream: u32,
    pub app: String,
    pub pid: Option<u32>,
    pub media: String,
    pub sid: Option<String>,
    pub encrypted: bool,
    /// 0 to 1, for a level meter.
    pub level: f64,
    /// Nothing heard yet 5 s after starting.
    pub silent: bool,
    /// Started with the screen share, and stops with it.
    pub attached: bool,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct ShareEnded {
    pub seq: u32,
    /// `"screen"` or `"audio"`.
    pub what: String,
    /// `"system"`, `"app-gone"` or `"error"`.
    pub reason: String,
    pub msg: String,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct ShareView {
    /// `"idle"`, `"starting"` (the picker is open or the first picture not here yet) or `"live"`.
    pub state: String,
    pub profile: Option<String>,
    /// `"portal"` or `"test"`.
    pub source: Option<String>,
    /// The size of the picture being sent, and frames per second pushed in the last second.
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub frames: u64,
    pub sid: Option<String>,
    pub encrypted: bool,
    /// A switch is waiting for a new screen to be picked.
    pub switching: bool,
    pub audio: Option<ShareAudioView>,
    pub ended: Option<ShareEnded>,
}

/// What the share task tells the engine.
pub enum Report {
    View(ShareView),
    Log(String),
    /// Something that did not start: a passing error for the dock.
    Fail(String),
}

// ------------------------------------------------------------------------------------------------ the task

#[derive(Clone)]
pub struct Sess { pub room: Arc<Room>, pub book: Arc<Mutex<KeyBook>> }

pub enum ShareCmd {
    Session(Option<Sess>),
    Start { profile: Profile, source: ShareSource, audio: Option<u32> },
    Switch(ShareSource),
    Profile(Profile),
    Stop,
    AudioStart(u32),
    AudioStop,
    /// Tests: the capture ends as if the system had closed it.
    TestEnd,
    /// Tests: the encoder's own numbers for each layer of the picture (outbound-rtp).
    Stats(tokio::sync::oneshot::Sender<serde_json::Value>),
    Audio(AudioMsg),
}
pub enum AudioMsg { Gone { gen: u64, msg: String } }

#[derive(Clone)]
pub struct ShareHandle { tx: UnboundedSender<ShareCmd> }
impl ShareHandle {
    pub fn send(&self, c: ShareCmd) { let _ = self.tx.send(c); }
}

pub fn spawn(rt: &tokio::runtime::Handle, report: Arc<dyn Fn(Report) + Send + Sync>) -> ShareHandle {
    let (tx, rx) = unbounded_channel();
    let task = Task::new(tx.clone(), report);
    rt.spawn(task.run(rx));
    ShareHandle { tx }
}

struct VidPub { src: NativeVideoSource, publication: LocalTrackPublication, w: u32, h: u32 }

struct AudioShare {
    gen: u64,
    app: AppStream,
    attached: bool,
    src: NativeAudioSource,
    publication: Option<LocalTrackPublication>,
    feeder: tokio::task::JoinHandle<()>,
    level: Arc<AtomicU32>,
    heard: Arc<AtomicBool>,
    started: Instant,
}

struct Task {
    me: UnboundedSender<ShareCmd>,
    report: Arc<dyn Fn(Report) + Send + Sync>,
    sess: Option<Sess>,
    profile: Profile,
    source: Option<ShareSource>,
    cap: Option<Capture>,
    /// A switch's new capture, until it brings its first picture.
    pending: Option<(Capture, ShareSource)>,
    vid: Option<VidPub>,
    resize: Option<(u32, u32, Instant)>,
    audio: Option<AudioShare>,
    audio_gen: u64,
    view: ShareView,
    last_sent: Option<ShareView>,
    sec_start: Instant,
    sec_frames: u32,
    ended_seq: u32,
    audio_check: Instant,
}

async fn next(c: Option<&mut Capture>) -> Option<Item> {
    match c { Some(c) => c.rx.recv().await, None => std::future::pending().await }
}

fn ts() -> i64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_micros() as i64).unwrap_or(0) }

impl Task {
    fn new(me: UnboundedSender<ShareCmd>, report: Arc<dyn Fn(Report) + Send + Sync>) -> Task {
        Task { me, report, sess: None, profile: Profile::Text, source: None, cap: None, pending: None, vid: None, resize: None, audio: None, audio_gen: 0,
            view: ShareView { state: "idle".into(), ..Default::default() }, last_sent: None, sec_start: Instant::now(), sec_frames: 0, ended_seq: 0, audio_check: Instant::now() }
    }

    async fn run(mut self, mut rx: UnboundedReceiver<ShareCmd>) {
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                c = rx.recv() => match c { Some(c) => self.cmd(c).await, None => break },
                it = next(self.cap.as_mut()) => self.on_item(it, false).await,
                it = next(self.pending.as_mut().map(|p| &mut p.0)) => self.on_item(it, true).await,
                _ = tick.tick() => self.on_tick(),
            }
            self.push_view();
        }
        self.stop_screen(false).await;
        self.stop_audio(false).await;
    }

    fn log(&self, m: impl Into<String>) { (self.report)(Report::Log(m.into())); }
    fn fail(&self, m: impl Into<String>) { (self.report)(Report::Fail(m.into())); }
    fn push_view(&mut self) {
        if self.last_sent.as_ref() != Some(&self.view) { self.last_sent = Some(self.view.clone()); (self.report)(Report::View(self.view.clone())); }
    }

    fn ended(&mut self, what: &str, reason: &str, msg: String) {
        self.ended_seq += 1;
        self.log(format!("{what} share ended ({reason}): {msg}"));
        self.view.ended = Some(ShareEnded { seq: self.ended_seq, what: what.into(), reason: reason.into(), msg });
    }

    async fn cmd(&mut self, c: ShareCmd) {
        match c {
            ShareCmd::Session(s) => self.on_session(s).await,
            ShareCmd::Start { profile, source, audio } => {
                if self.sess.is_none() { self.fail("Join the call first."); return; }
                if self.cap.is_some() || self.pending.is_some() || self.vid.is_some() { self.stop_screen(true).await; }
                self.view.ended = None;
                self.profile = profile;
                self.source = Some(source);
                self.view.profile = Some(profile.name().into());
                self.view.source = Some(source.name().into());
                self.view.state = "starting".into();
                self.cap = Some(capture::start(source, Limits::of(profile)));
                self.log(format!("screen share starting: {} profile, {} source", profile.name(), source.name()));
                if let Some(id) = audio { self.start_audio(id, true).await; }
            }
            ShareCmd::Switch(source) => {
                if self.cap.is_none() { self.fail("There is no screen share to switch."); return; }
                self.pending = Some((capture::start(source, Limits::of(self.profile)), source));
                self.view.switching = true;
                self.log(format!("choosing another source: {}", source.name()));
            }
            ShareCmd::Profile(p) => {
                if self.cap.is_none() || p == self.profile { self.profile = p; return; }
                self.profile = p;
                self.view.profile = Some(p.name().into());
                let l = Limits::of(p);
                if let Some(c) = &self.cap { c.set_limits(l); }
                if let Some((c, _)) = &self.pending { c.set_limits(l); }
                self.unpublish_video().await;   // republished by the next picture with the new encodings
                self.view.state = "starting".into();
                self.log(format!("share profile: {}", p.name()));
            }
            ShareCmd::Stop => {
                self.stop_screen(true).await;
                self.view.ended = None;
                if self.audio.as_ref().is_some_and(|a| a.attached) { self.stop_audio(true).await; }
            }
            ShareCmd::AudioStart(id) => {
                if self.sess.is_none() { self.fail("Join the call first."); return; }
                self.view.ended = None;
                self.start_audio(id, false).await;
            }
            ShareCmd::AudioStop => { self.stop_audio(true).await; }
            ShareCmd::TestEnd => { if let Some(c) = &self.cap { c.die(); } }
            ShareCmd::Stats(reply) => {
                let t = self.vid.as_ref().and_then(|v| v.publication.track());
                tokio::spawn(async move {
                    let v = match t {
                        Some(LocalTrack::Video(t)) => match t.get_stats().await {
                            Ok(list) => serde_json::json!({"layers": list.into_iter().filter_map(|s| match s {
                                livekit::webrtc::stats::RtcStats::OutboundRtp(o) => Some(serde_json::json!({"rid": o.outbound.rid, "w": o.outbound.frame_width, "h": o.outbound.frame_height, "fps": o.outbound.frames_per_second,
                                    "encoded": o.outbound.frames_encoded, "sent": o.outbound.frames_sent, "bytes": o.sent.bytes_sent, "limit": format!("{:?}", o.outbound.quality_limitation_reason), "target": o.outbound.target_bitrate})),
                                _ => None }).collect::<Vec<_>>()}),
                            Err(e) => serde_json::json!({"error": e.to_string()}),
                        },
                        _ => serde_json::json!({"layers": []}),
                    };
                    let _ = reply.send(v);
                });
            }
            ShareCmd::Audio(AudioMsg::Gone { gen, msg }) => {
                if self.audio.as_ref().is_some_and(|a| a.gen == gen) {
                    self.stop_audio(true).await;
                    self.ended("audio", "app-gone", msg);
                }
            }
        }
    }

    /// The engine built (or lost) a session: what was published belongs to the old room, so it is published again from the
    /// still-running capture and tap.
    async fn on_session(&mut self, s: Option<Sess>) {
        self.vid = None;   // the old room is gone with its tracks
        self.resize = None;
        if let Some(a) = &mut self.audio { a.publication = None; }
        self.sess = s;
        if self.sess.is_some() {
            if self.cap.is_some() { self.view.state = "starting".into(); }
            self.publish_audio().await;
        } else if self.cap.is_some() { self.view.state = "starting".into(); }
        self.view.sid = None; self.view.encrypted = false;
    }

    // -------------------------------------------------------------------------------------------- pictures
    async fn on_item(&mut self, it: Option<Item>, pending: bool) {
        match it {
            Some(Item::Frame(b)) => {
                if pending {
                    // the new screen was picked: it takes over from the old one
                    if let Some((c, s)) = self.pending.take() {
                        self.cap = Some(c); self.source = Some(s); self.view.source = Some(s.name().into()); self.view.switching = false;
                        self.log("switched to the new source");
                    }
                }
                self.on_frame(b).await;
            }
            Some(Item::Ended { msg, system }) => {
                if pending {
                    self.pending = None; self.view.switching = false;
                    self.log(format!("the switch was called off: {msg}"));
                    self.fail(msg);
                } else {
                    let reason = if system { "system" } else { "error" };
                    self.stop_screen(true).await;
                    self.ended("screen", reason, msg.clone());
                    if !system { self.fail(msg); }
                    if self.audio.as_ref().is_some_and(|a| a.attached) { self.stop_audio(true).await; }
                }
            }
            None => {
                if pending { self.pending = None; self.view.switching = false; }
                else { self.stop_screen(true).await; self.ended("screen", "error", "The capture stopped.".into()); }
            }
        }
    }

    async fn on_frame(&mut self, b: livekit::webrtc::video_frame::I420Buffer) {
        let (w, h) = (b.width(), b.height());
        self.sec_frames += 1;
        self.view.frames += 1;
        let Some(sess) = self.sess.clone() else { return };   // no room right now: the picture is dropped
        let same = self.vid.as_ref().is_some_and(|v| v.w == w && v.h == h);
        if !same {
            if self.vid.is_some() {
                // a new size (another screen, or a resized window): republish once it holds still for a moment
                match self.resize { Some((rw, rh, t)) if (rw, rh) == (w, h) => { if t.elapsed() < Duration::from_millis(700) { return; } } _ => { self.resize = Some((w, h, Instant::now())); return; } }
                self.unpublish_video().await;
            }
            if let Err(e) = self.publish_video(&sess, w, h).await {
                self.stop_screen(true).await;
                self.ended("screen", "error", e.clone());
                self.fail(e);
                if self.audio.as_ref().is_some_and(|a| a.attached) { self.stop_audio(true).await; }
                return;
            }
        }
        self.resize = None;
        if let Some(v) = &self.vid {
            let mut f = VideoFrame::new(VideoRotation::VideoRotation0, b);
            f.timestamp_us = ts();
            v.src.capture_frame(&f);
        }
        self.view.width = w; self.view.height = h; self.view.state = "live".into();
    }

    async fn publish_video(&mut self, sess: &Sess, w: u32, h: u32) -> Result<(), String> {
        let spec = self.profile.spec();
        let src = NativeVideoSource::new(VideoResolution { width: w, height: h }, spec.screencast);
        let track = LocalVideoTrack::create_video_track("screen", RtcVideoSource::Native(src.clone()));
        let p = sess.room.local_participant().publish_track(LocalTrack::Video(track), self.profile.publish_options()).await.map_err(|e| format!("The screen share could not be published: {e}"))?;
        if p.encryption_type() == EncryptionType::None {
            let _ = sess.room.local_participant().unpublish_track(&p.sid()).await;
            return Err("The server did not mark your screen share as encrypted, so it was stopped. Nothing was sent.".into());
        }
        sess.book.lock().point_senders(sess.room.e2ee_manager());
        self.view.sid = Some(p.sid().to_string());
        self.view.encrypted = true;
        self.log(format!("screen share published: {w}x{h}, {} profile", self.profile.name()));
        self.vid = Some(VidPub { src, publication: p, w, h });
        Ok(())
    }

    async fn unpublish_video(&mut self) {
        if let (Some(v), Some(s)) = (self.vid.take(), self.sess.as_ref()) { let _ = s.room.local_participant().unpublish_track(&v.publication.sid()).await; }
        self.vid = None; self.resize = None; self.view.sid = None; self.view.encrypted = false;
    }

    /// Ends the picture: capture, pending switch and track. (`quiet`: the room is still there to tell.)
    async fn stop_screen(&mut self, _quiet: bool) {
        self.cap = None; self.pending = None; self.source = None;
        self.unpublish_video().await;
        let (profile, audio) = (self.view.profile.take(), self.view.audio.take());
        self.view = ShareView { state: "idle".into(), ended: self.view.ended.take(), audio, ..Default::default() };
        let _ = profile;
    }

    // -------------------------------------------------------------------------------------------- one app's sound
    async fn start_audio(&mut self, id: u32, attached: bool) {
        self.stop_audio(true).await;
        let found = tokio::task::spawn_blocking(move || appaudio::find(id)).await.unwrap_or_else(|e| Err(e.to_string()));
        let app = match found { Ok(a) => a, Err(e) => { self.fail(e); return; } };
        let tap = match appaudio::tap(&app) { Ok(t) => t, Err(e) => { self.fail(e); return; } };
        self.audio_gen += 1;
        let gen = self.audio_gen;
        let src = NativeAudioSource::new(AudioSourceOptions { echo_cancellation: false, noise_suppression: false, auto_gain_control: false }, 48000, 2, 100);
        let (level, heard) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicBool::new(false)));
        let (s2, l2, h2, me, name) = (src.clone(), level.clone(), heard.clone(), self.me.clone(), app.app.clone());
        let feeder = tokio::spawn(async move {
            let mut tap = tap;
            while let Some(chunk) = tap.rx.recv().await {
                let peak = chunk.iter().map(|s| s.unsigned_abs() as u32).max().unwrap_or(0);
                if peak > 16 { h2.store(true, Ordering::Relaxed); }
                // fast attack, slow release, in 1/32768ths
                let cur = l2.load(Ordering::Relaxed);
                l2.store(if peak > cur { peak } else { cur - (cur - peak) / 25 }, Ordering::Relaxed);
                let f = AudioFrame { data: chunk.into(), sample_rate: 48000, num_channels: 2, samples_per_channel: 480 };
                if s2.capture_frame(&f).await.is_err() { break; }
            }
            let _ = me.send(ShareCmd::Audio(AudioMsg::Gone { gen, msg: format!("{name} stopped playing sound, so its sound is no longer shared.") }));
        });
        self.log(format!("sharing the sound of {} (stream {id}, pid {:?})", app.app, app.pid));
        self.audio = Some(AudioShare { gen, app, attached, src, publication: None, feeder, level, heard, started: Instant::now() });
        self.publish_audio().await;
        self.update_audio_view();
    }

    async fn publish_audio(&mut self) {
        let (Some(sess), Some(a)) = (self.sess.clone(), self.audio.as_mut()) else { return };
        if a.publication.is_some() { return; }
        let track = LocalAudioTrack::create_audio_track(AUDIO_TRACK_NAME, RtcAudioSource::Native(a.src.clone()));
        match sess.room.local_participant().publish_track(LocalTrack::Audio(track), audio_publish_options()).await {
            Ok(p) => {
                if p.encryption_type() == EncryptionType::None {
                    let _ = sess.room.local_participant().unpublish_track(&p.sid()).await;
                    self.fail("The server did not mark the shared sound as encrypted, so it was stopped. Nothing was sent.");
                    self.stop_audio(true).await;
                    return;
                }
                sess.book.lock().point_senders(sess.room.e2ee_manager());
                a.publication = Some(p);
            }
            Err(e) => { let m = format!("The sound could not be published: {e}"); self.fail(m); self.stop_audio(true).await; }
        }
    }

    async fn stop_audio(&mut self, _quiet: bool) {
        let Some(a) = self.audio.take() else { return };
        a.feeder.abort();
        if let (Some(p), Some(s)) = (a.publication, self.sess.as_ref()) { let _ = s.room.local_participant().unpublish_track(&p.sid()).await; }
        self.view.audio = None;
    }

    fn update_audio_view(&mut self) {
        self.view.audio = self.audio.as_ref().map(|a| ShareAudioView {
            stream: a.app.id, app: a.app.app.clone(), pid: a.app.pid, media: a.app.media.clone(),
            sid: a.publication.as_ref().map(|p| p.sid().to_string()), encrypted: a.publication.is_some(),
            level: (a.level.load(Ordering::Relaxed) as f64 / 32768.0 * 100.0).round() / 100.0,
            silent: !a.heard.load(Ordering::Relaxed) && a.started.elapsed() > Duration::from_secs(5), attached: a.attached,
        });
    }

    fn on_tick(&mut self) {
        let e = self.sec_start.elapsed();
        if e >= Duration::from_secs(1) {
            self.view.fps = (self.sec_frames as f64 / e.as_secs_f64() * 10.).round() / 10.;
            self.sec_frames = 0; self.sec_start = Instant::now();
            if self.cap.is_none() { self.view.fps = 0.; }
        }
        self.update_audio_view();
        // the capture does not end by itself when the app closes its stream (the sink's monitor goes on, silent), so the
        // stream is looked for every 2 s
        if let Some(a) = &self.audio {
            if self.audio_check.elapsed() >= Duration::from_secs(2) {
                self.audio_check = Instant::now();
                let (id, gen, me) = (a.app.id, a.gen, self.me.clone());
                tokio::task::spawn_blocking(move || {
                    if let Ok(list) = appaudio::list() {
                        if !list.iter().any(|s| s.id == id) { let _ = me.send(ShareCmd::Audio(AudioMsg::Gone { gen, msg: "The app stopped playing sound, so its sound is no longer shared.".into() })); }
                    }
                });
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use livekit::options::compute_video_encodings;

    fn enc(p: Profile, w: u32, h: u32) -> Vec<(String, u64, f64, f64)> {
        compute_video_encodings(w, h, &p.publish_options()).iter().map(|e| (e.rid.clone(), e.max_bitrate.unwrap_or(0), e.max_framerate.unwrap_or(0.), e.scale_resolution_down_by.unwrap_or(1.))).collect()
    }

    #[test]
    fn every_profile_is_the_full_picture_and_one_540p_layer() {
        for (p, bitrate, fps, lb, lf) in [(Profile::Text, 6_000_000, 30., 350_000, 5.), (Profile::Motion, 6_000_000, 30., 600_000, 15.), (Profile::Motion60, 9_000_000, 60., 600_000, 15.)] {
            let (w, h) = if p == Profile::Text { (2560, 1440) } else { (1920, 1080) };
            let e = enc(p, w, h);
            assert_eq!(e.len(), 2, "{p:?}: two encodings, the 540p layer and the full picture");
            // the SDK lists the full picture first, then the layer (rids "h" and "q": the viewer's HIGH and LOW)
            assert_eq!((e[0].0.as_str(), e[1].0.as_str()), ("h", "q"));
            assert_eq!((e[1].1, e[1].2), (lb, lf), "{p:?} layer");
            assert!((e[1].3 - h as f64 / 540.).abs() < 0.01, "{p:?} layer is scaled down to 540 high: {}", e[1].3);
            assert_eq!((e[0].1, e[0].2, e[0].3), (bitrate, fps, 1.), "{p:?} full picture");
        }
    }

    #[test]
    fn options_match_the_web_profiles() {
        let o = Profile::Text.publish_options();
        assert_eq!(o.source, TrackSource::Screenshare);
        assert_eq!(o.video_codec, VideoCodec::VP8);
        assert!(o.simulcast);
        assert_eq!(o.simulcast_layers.as_ref().map(|l| l.len()), Some(1));
        assert_eq!(o.degradation_preference, Some(DegradationPreference::MaintainResolution));
        assert!(Profile::Text.spec().screencast && !Profile::Motion.spec().screencast);
        assert_eq!(Profile::Motion.publish_options().degradation_preference, Some(DegradationPreference::MaintainFramerate));
        let a = audio_publish_options();
        assert_eq!((a.source, a.dtx, a.audio_encoding.map(|e| e.max_bitrate)), (TrackSource::ScreenshareAudio, false, Some(192_000)));
        assert_eq!(AUDIO_TRACK_NAME, "music-audio");
    }

    #[test]
    fn profile_names_round_trip() {
        for p in [Profile::Text, Profile::Motion, Profile::Motion60] { assert_eq!(Profile::parse(p.name()), Some(p)); }
        assert_eq!(Profile::parse("nope"), None);
    }
}
