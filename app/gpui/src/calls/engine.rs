//! The call engine (CALLS-PLAN.md section 8, plus step 3's voice): one room's call, without any window. call.js ported
//! with the same constants: join, keys, who may be heard, heartbeat, reconnects, mute, deafen, game keys, speaking,
//! and now the voice controls: audio devices, the three processing switches, push-to-talk and per-person volume.
//!
//! # The UI's API
//!
//! - **Start**: [`CallEngine::start`] with the core link ([`CoreLink`]) and [`Options`] (audio mode, settings).
//! - **Do things**: [`CallEngine::send`] a [`Command`]. Everything the engine does is a command.
//! - **Read state**: [`CallEngine::snapshot`] returns a [`Snapshot`] (`phase`, `error`, `peers`, `devices`, `muted`,
//!   `deafened`, `echo_cancellation`/`noise_suppression`/`auto_gain_control`, `push_to_talk`, `ptt_held`, per-peer
//!   `volume`, …).
//! - **Wait for changes**: [`CallEngine::changes`] is a `watch` channel bumped (at most once per 60 ms) whenever the
//!   snapshot changed. Poll `snapshot()` after each bump; do not poll on a timer.
//! - **Device lists** are on demand: send [`Command::ListDevices`], wait for the change, read `snapshot().devices`.
//! - **Screen share and one app's sound** (step 5): `Command::StartShare` / `StopShare` / `SwitchShareSource` /
//!   `SetShareProfile` / `ListShareApps` / `StartShareAudio` / `StopShareAudio`, read from `Snapshot.share` and
//!   `Snapshot.share_apps`. What every field and event means, and how it is published, is at the top of
//!   [`crate::calls::share`].
//! - **Tests only**: [`CallEngine::plout`] (the receiver's playout energy, the W1 measure) and
//!   [`CallEngine::mic_options`] (what the published microphone source actually runs).
//!
//! The UI keeps nothing of its own; every choice is remembered by the engine in [`crate::calls::settings::CallSettings`]
//! (per device, beside the account) and applied again on the next join.
//!
//! Shape: an actor on a tokio runtime of its own (2 threads). The UI sends [`Command`]s and reads a [`Snapshot`], which
//! is rebuilt at most every 60 ms when something changed. Core calls are blocking and go to dedicated threads, never a
//! pool (SPIKE.md: a pool keeps a heap per thread): one for the call's own calls, one for the game-key long poll, which
//! would otherwise hold the 1.5 s heartbeat up for its 4 s.
use super::audio::{self, Devices, Processing};
use super::errors::{explain, removed_from_room, removed_notice, token_refused, CallError, Kind, PASSING_FADE};
use super::gate::{MicGate, Publish};
use super::keys::{self, KeyBook};
use super::reconnect::{self as rc, MediaWatch};
use super::settings::CallSettings;
use super::appaudio::AppStream;
use super::capture::ShareSource;
use super::share::{self, Profile, Report, Sess, ShareCmd, ShareHandle, ShareView};
use super::{ice, review, tone};
use futures::StreamExt;
use livekit::e2ee::{E2eeOptions, EncryptionType};
use livekit::options::TrackPublishOptions;
use livekit::prelude::*;
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::{native::NativeAudioSource, RtcAudioSource};
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

// ------------------------------------------------------------------------------------------------ the core

/// The core's JSON API (`App::call`). Blocking.
pub trait CoreLink: Send + Sync + 'static {
    fn call(&self, method: &str, args: Value) -> Result<Value, String>;
    /// True while the core counts a call as running (its `call_active`), for checks.
    fn call_active(&self) -> bool { false }
}

/// One dedicated thread for blocking core calls.
#[derive(Clone)]
pub struct Lane { tx: std::sync::mpsc::Sender<Box<dyn FnOnce() + Send>> }
impl Lane {
    pub fn new(name: &str) -> Lane {
        let (tx, rx) = std::sync::mpsc::channel::<Box<dyn FnOnce() + Send>>();
        std::thread::Builder::new().name(name.into()).spawn(move || { while let Ok(job) = rx.recv() { job(); } }).expect("thread");
        Lane { tx }
    }
    pub async fn call(&self, core: &Arc<dyn CoreLink>, method: &str, args: Value) -> Result<Value, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (core, method) = (core.clone(), method.to_string());
        if self.tx.send(Box::new(move || { let _ = tx.send(core.call(&method, args)); })).is_err() { return Err("the core thread is gone".into()); }
        rx.await.unwrap_or_else(|_| Err("the core thread is gone".into()))
    }
}

// ------------------------------------------------------------------------------------------------ public face

#[derive(Clone, Debug)]
pub enum AudioMode {
    /// No audio device is opened (`MEGA_GPUI_CALL_AUDIO=none`): the microphone is a sine, hearing is measured on taps.
    Silent { tone_hz: f64 },
    /// libwebrtc's own device module (CALLS-PLAN.md step 3).
    Platform,
}
impl AudioMode {
    pub fn from_env() -> AudioMode {
        if std::env::var("MEGA_GPUI_CALL_AUDIO").as_deref() == Ok("none") {
            AudioMode::Silent { tone_hz: std::env::var("MEGA_GPUI_CALL_TONE").ok().and_then(|x| x.parse().ok()).unwrap_or(900.) }
        } else { AudioMode::Platform }
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    pub audio: AudioMode,
    /// The remembered choices (devices, processing, push-to-talk, volumes). Loaded by the caller from
    /// `settings::CallSettings::load`; the engine re-saves it to `settings_path` on every change.
    pub settings: CallSettings,
    pub settings_path: Option<PathBuf>,
}

#[derive(Debug)]
pub enum Command {
    Join { gid: String, name: String },
    Leave,
    SetMuted(bool),
    SetDeafened(bool),
    /// Per account, 0 to 10 (1 unchanged), applied to every device of that account (patch W1).
    SetVolume { account: String, volume: f64 },
    /// Fill `Snapshot.devices` from the platform's microphone and speaker lists, then raise the change.
    ListDevices,
    /// Choose the recording (microphone) device and switch to it live; remembered in settings.
    SetMicDevice { id: String },
    /// Choose the playout (speaker) device and switch to it live; remembered in settings.
    SetOutDevice { id: String },
    /// The three processing switches, applied to the microphone source (a change republishes the mic, never the room).
    SetProcessing(Processing),
    /// Enable or disable push-to-talk mode (the preference). While on, the mic follows press/release.
    SetPushToTalk(bool),
    /// A push-to-talk key went down (`true`) or up (`false`); ignored while PTT is off or the call is deafened.
    PushToTalk(bool),
    /// Tests: a game key as the core's `game_mute_wait` would report it; `None` hands the mic back to the real watcher.
    TestGame(Option<bool>),
    /// Share a screen (the system's picker opens for [`ShareSource::Portal`]), optionally with one app's sound (an id from
    /// `Snapshot.share_apps`). Refused outside a call. Starting again replaces the running share.
    StartShare { profile: Profile, source: ShareSource, audio: Option<u32> },
    /// End the share (and its sound, if it was started with it).
    StopShare,
    /// Pick another screen or window; the old share keeps running until a new one is chosen.
    SwitchShareSource(ShareSource),
    /// Change the profile of the running share (republishes; the capture is not touched).
    SetShareProfile(Profile),
    /// Fill `Snapshot.share_apps` with the apps playing sound now, then raise the change.
    ListShareApps,
    /// Publish one app's sound (a stream id from `share_apps`) as a second audio track, with or without a share.
    StartShareAudio { stream: u32 },
    StopShareAudio,
    /// Tests: the running capture ends as if the system had closed it.
    TestShareEnd,
    /// Tests: play every encrypted track whoever sends it, so a test can show the cryptography alone keeps a removed member out.
    TestTrustAll(bool),
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct ErrorView { pub kind: String, pub msg: String }
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Peer { pub identity: String, pub me: bool, pub name: String, pub account: Option<String>, pub member: bool, pub speaking: bool, pub muted: bool, pub deafened: bool, pub volume: f64, #[serde(rename = "cantDecrypt")] pub cant_decrypt: bool }
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Encryption { pub active: bool, pub epoch: Option<u64>, #[serde(rename = "keyIndex")] pub key_index: Option<i32>, pub rotations: u32, pub devices: usize, pub wipes: u32, #[serde(rename = "senderIndex")] pub sender_index: Vec<i32> }
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Blocked { pub identity: String, pub source: String, pub why: String }
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Snapshot {
    pub phase: String,
    pub error: Option<ErrorView>,
    pub gid: Option<String>,
    #[serde(rename = "roomName")] pub room_name: String,
    pub muted: bool,
    #[serde(rename = "gameMuted")] pub game_muted: bool,
    pub deafened: bool,
    pub peers: Vec<Peer>,
    pub encryption: Encryption,
    pub blocked: Vec<Blocked>,
    pub subscribed: Vec<String>,
    pub attempt: u32,
    pub events: Vec<String>,
    /// The platform's microphones and speakers, filled on [`Command::ListDevices`] (empty in the silent test mode).
    pub devices: Devices,
    /// The remembered recording and playout devices (settings), the ones actually in use while in a call.
    #[serde(rename = "micId")] pub mic_id: Option<String>,
    #[serde(rename = "outId")] pub out_id: Option<String>,
    #[serde(rename = "echoCancellation")] pub echo_cancellation: bool,
    #[serde(rename = "noiseSuppression")] pub noise_suppression: bool,
    #[serde(rename = "autoGainControl")] pub auto_gain_control: bool,
    #[serde(rename = "pushToTalk")] pub push_to_talk: bool,
    #[serde(rename = "pttHeld")] pub ptt_held: bool,
    /// The screen share and the shared app's sound; see [`crate::calls::share`].
    pub share: ShareView,
    /// The apps playing sound, filled on [`Command::ListShareApps`].
    #[serde(rename = "shareApps")] pub share_apps: Vec<AppStream>,
}

/// Tests: tone meters on the subscribed microphones.
#[derive(Default)]
pub struct Taps { meters: Mutex<Vec<(String, u64, tone::ToneMeter)>>, next: std::sync::atomic::AtomicU64 }
impl Taps {
    fn push(&self, identity: &str, f: &AudioFrame) {
        for (who, _, m) in self.meters.lock().iter_mut() { if who == identity { m.push(&f.data, f.sample_rate, f.num_channels); } }
    }
    /// The best tone share of one sender's microphone over `ms`, as call.js's `probeTone` reports it.
    pub async fn hear(&self, identity: &str, hz: f64, ms: u64) -> Value {
        let id = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.meters.lock().push((identity.to_string(), id, tone::ToneMeter::new(hz)));
        tokio::time::sleep(Duration::from_millis(ms)).await;
        let mut g = self.meters.lock();
        let i = g.iter().position(|(_, n, _)| *n == id).unwrap();
        let (_, _, m) = g.remove(i);
        if m.frames == 0 { json!({"present": false}) } else { m.result() }
    }
}

pub struct CallEngine {
    tx: UnboundedSender<Msg>,
    snap: Arc<Mutex<Snapshot>>,
    version: tokio::sync::watch::Receiver<u64>,
    pub taps: Arc<Taps>,
    pub rt: Arc<tokio::runtime::Runtime>,
    pub core: Arc<dyn CoreLink>,
}

impl CallEngine {
    pub fn start(core: Arc<dyn CoreLink>, opts: Options) -> CallEngine {
        let rt = Arc::new(tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("mega-call").enable_all().build().expect("tokio runtime"));
        let (tx, rx) = unbounded_channel();
        let snap: Arc<Mutex<Snapshot>> = Arc::new(Mutex::new(Snapshot { phase: "idle".into(), ..Default::default() }));
        let (vtx, version) = tokio::sync::watch::channel(0u64);
        let taps: Arc<Taps> = Default::default();
        let actor = Actor::new(core.clone(), opts, tx.clone(), snap.clone(), vtx, taps.clone());
        rt.spawn(actor.run(rx));
        CallEngine { tx, snap, version, taps, rt, core }
    }
    pub fn send(&self, c: Command) { let _ = self.tx.send(Msg::Cmd(c)); }
    pub fn snapshot(&self) -> Snapshot { self.snap.lock().clone() }
    /// Bumped whenever the snapshot changed (at most every 60 ms).
    pub fn changes(&self) -> tokio::sync::watch::Receiver<u64> { self.version.clone() }

    /// Tests: the receiver's playout energy of one sender's microphone over `ms` (inbound-rtp `totalAudioEnergy`, the
    /// W1 measure; a tap would sit before the gain and miss it). `present: false` when that sender has no microphone
    /// subscribed here.
    pub async fn plout(&self, identity: &str, ms: u64) -> Value {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if self.tx.send(Msg::Plout { identity: identity.to_string(), ms, reply: tx }).is_err() { return json!({"present": false}); }
        rx.await.unwrap_or_else(|_| json!({"error": "the engine is gone"}))
    }

    /// Tests: the encoder's numbers per layer of the shared picture (rid, size, frames encoded and sent, quality limit).
    pub async fn share_stats(&self) -> Value {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if self.tx.send(Msg::ShareStats { reply: tx }).is_err() { return json!({"layers": []}); }
        rx.await.unwrap_or_else(|_| json!({"error": "the engine is gone"}))
    }

    /// Tests: what the published microphone source actually runs (its `AudioSourceOptions`). In the silent test mode
    /// the source follows the processing switches, so this shows a switch reaching the SDK.
    pub async fn mic_options(&self) -> Value {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if self.tx.send(Msg::MicOptions { reply: tx }).is_err() { return json!({"present": false}); }
        rx.await.unwrap_or_else(|_| json!({"error": "the engine is gone"}))
    }
}

// ------------------------------------------------------------------------------------------------ the actor

#[derive(Clone, Debug)]
struct Token { url: String, token: String, expires: u64, turn_port: Option<u16> }
impl Token {
    fn parse(v: &Value) -> Result<Token, CallError> {
        let s = |k: &str| v[k].as_str().map(String::from).ok_or_else(|| CallError::new(Kind::Other, format!("call_token: no {k}")));
        Ok(Token { url: s("url")?, token: s("token")?, expires: v["expires"].as_u64().unwrap_or(0), turn_port: v["turn_port"].as_u64().map(|p| p as u16) })
    }
}

#[derive(Clone, Copy, Debug)]
enum Attempt { Join(usize), Rejoin(usize) }

struct Conn { room: Arc<Room>, events: UnboundedReceiver<RoomEvent>, book: KeyBook, st: keys::CallState, tok: Token }

enum GameNews { State { seq: u64, muted: bool, game: Option<String> }, Lost(String) }

enum Msg {
    Cmd(Command),
    Connected { gen: u64, attempt: Attempt, result: Result<Conn, CallError> },
    Polled { call: u64, result: Result<Value, String> },
    Game { call: u64, news: GameNews },
    Mic { call: u64, result: Result<(), String> },
    Devices { call: u64, result: Result<Devices, String> },
    Device { call: u64, result: Result<(), String> },
    Plout { identity: String, ms: u64, reply: tokio::sync::oneshot::Sender<Value> },
    MicOptions { reply: tokio::sync::oneshot::Sender<Value> },
    ShareStats { reply: tokio::sync::oneshot::Sender<Value> },
    Room { gen: u64, ev: RoomEvent },
    Share(Report),
    ShareApps(Result<Vec<AppStream>, String>),
}

struct Session { room: Arc<Room>, book: Arc<Mutex<KeyBook>>, gen: u64 }

struct TrackInfo { identity: String, source: TrackSource, audio: Option<RemoteAudioTrack> }

/// What the audio should be doing right now. The gate owns `on` (mute/unmute); the engine owns the device choices and
/// the processing switches. [`realize_mic`] reconciles this against the realised [`MicSlot`], so whoever changed the
/// wish (a mute press, a device pick, a processing switch) ends up with the same final microphone.
#[derive(Clone, Debug, Default, PartialEq)]
struct AudioWish { on: bool, processing: Processing, mic_id: Option<String>, out_id: Option<String> }

/// The realised audio of the current session.
#[derive(Default)]
struct MicSlot {
    gen: u64,
    room: Option<Arc<Room>>,
    book: Option<Arc<Mutex<KeyBook>>>,
    publication: Option<LocalTrackPublication>,
    feeder: Option<tokio::task::JoinHandle<()>>,
    platform: Option<livekit::PlatformAudio>,
    realized: AudioWish,
}

/// Runs a blocking audio-device-module call on the blocking pool, so a slow or stuck ADM (PulseAudio/PipeWire) can
/// never starve the engine's two tokio workers (the plan's rule: blocking calls never run on the actor or its workers).
/// A stuck call is abandoned after [`ADM_TIMEOUT`] so it can never hold the microphone slot open for ever.
const ADM_TIMEOUT: Duration = Duration::from_secs(5);
async fn adms<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    match tokio::time::timeout(ADM_TIMEOUT, tokio::task::spawn_blocking(f)).await {
        Ok(Ok(t)) => Ok(t),
        Ok(Err(e)) => Err(format!("audio device thread: {e}")),
        Err(_) => Err("the audio device did not answer in time".into()),
    }
}

/// Makes the microphone and the platform's devices match `wish`. Device switches are hot-swaps on the SDK's device
/// module (no republish, so the call is never dropped); a processing change republishes the microphone, because
/// libwebrtc bakes the software AEC/NS/AGC into the source when it is created. Muting keeps the track but turns
/// recording off, so the system's microphone indicator goes off too (call.js `stopRecording`/`startRecording`).
async fn realize_mic(slot: Arc<tokio::sync::Mutex<MicSlot>>, mode: AudioMode, wish: Arc<parking_lot::Mutex<AudioWish>>) -> Result<(), String> {
    let mut s = slot.lock().await;
    let Some(room) = s.room.clone() else { return Err("not in a call".into()) };
    let w = wish.lock().clone();

    // device changes: switch on the device module in place
    if w.out_id != s.realized.out_id {
        if let Some(id) = &w.out_id {
            if let Some(p) = &s.platform {
                let (p, id) = (p.clone(), audio::playout_id(id));
                adms(move || p.switch_playout_device(&id)).await?.map_err(|e| format!("speaker device: {e}"))?;
            }
        }
        s.realized.out_id = w.out_id.clone();
    }
    if w.mic_id != s.realized.mic_id {
        if let Some(id) = &w.mic_id {
            if let Some(p) = &s.platform {
                let (p, id) = (p.clone(), audio::recording_id(id));
                adms(move || p.switch_recording_device(&id)).await?.map_err(|e| format!("microphone device: {e}"))?;
            }
        }
        s.realized.mic_id = w.mic_id.clone();
    }

    // processing change: the options are part of the source, so the mic is republished (never the room)
    if w.processing != s.realized.processing {
        if let Some(p) = s.publication.take() {
            let sid = p.sid();
            let _ = room.local_participant().unpublish_track(&sid).await;
            if let Some(f) = s.feeder.take() { f.abort(); }
        }
        s.realized.processing = w.processing;
    }

    if !w.on {
        if let Some(p) = &s.publication { p.mute(); }
        if let Some(p) = &s.platform { let p = p.clone(); let _ = adms(move || p.stop_recording()).await; }
        return Ok(());
    }
    if let Some(p) = &s.publication {
        p.unmute();
        if let Some(p) = &s.platform { let p = p.clone(); let _ = adms(move || p.start_recording()).await; }
        return Ok(());
    }

    // create and publish a fresh microphone
    let (feeder_src, platform_source) = match &mode {
        AudioMode::Silent { .. } => {
            let src = NativeAudioSource::new(w.processing.into(), 48000, 1, 100);
            (Some(src), None)
        }
        AudioMode::Platform => {
            if s.platform.is_none() {
                let p = adms(livekit::PlatformAudio::new).await?.map_err(|e| format!("microphone: {e}"))?;
                // apply the remembered devices before the first publish; an id that is gone falls back to the default
                if let Some(id) = &w.out_id { let (p2, id) = (p.clone(), audio::playout_id(id)); let _ = adms(move || p2.set_playout_device(&id)).await; }
                if let Some(id) = &w.mic_id { let (p2, id) = (p.clone(), audio::recording_id(id)); let _ = adms(move || p2.set_recording_device(&id)).await; }
                s.platform = Some(p);
            }
            let p = s.platform.as_ref().unwrap();
            (None, Some(p.rtc_source()))
        }
    };
    let track = match platform_source {
        Some(_) => LocalAudioTrack::create_device_audio_track_with_options("microphone", w.processing.into()),
        None => LocalAudioTrack::create_audio_track("microphone", RtcAudioSource::Native(feeder_src.clone().unwrap())),
    };
    let p = room.local_participant().publish_track(LocalTrack::Audio(track), TrackPublishOptions { source: TrackSource::Microphone, dtx: true, ..Default::default() }).await.map_err(|e| format!("microphone: {e}"))?;
    if p.encryption_type() == EncryptionType::None {
        let _ = room.local_participant().unpublish_track(&p.sid()).await;
        return Err("no-e2ee: The server did not mark your microphone track as encrypted. The microphone was switched off.".into());
    }
    // rule 3 before any sound is fed (patch L2 already made the cryptor on this index)
    if let Some(b) = &s.book { b.lock().point_senders(room.e2ee_manager()); }
    if let (Some(src), AudioMode::Silent { tone_hz }) = (feeder_src, mode) {
        s.feeder = Some(tokio::spawn(async move {
            let mut t = tokio::time::interval(Duration::from_millis(10));
            let mut n = 0u64;
            loop { t.tick().await; let d = tone::sine(tone_hz, &mut n, 12000.); let _ = src.capture_frame(&AudioFrame { data: d.into(), sample_rate: 48000, num_channels: 1, samples_per_channel: 480 }).await; }
        }));
    }
    if let Some(p) = &s.platform { let p = p.clone(); let _ = adms(move || p.start_recording()).await; }
    s.realized = w;
    s.publication = Some(p);
    Ok(())
}

struct Actor {
    core: Arc<dyn CoreLink>,
    lane: Lane,
    game_lane: Lane,
    opts: Options,
    tx: UnboundedSender<Msg>,
    snap_out: Arc<Mutex<Snapshot>>,
    version: tokio::sync::watch::Sender<u64>,
    taps: Arc<Taps>,

    gid: Option<String>,
    room_name: String,
    phase: &'static str,
    error: Option<(CallError, Instant)>,
    events: VecDeque<String>,
    call: u64,
    gen: u64,
    session: Option<Session>,
    token: Option<Token>,
    attempt: u32,
    retry_at: Option<(Instant, Attempt)>,
    rejoin_started: Option<Instant>,
    reconnecting_since: Option<Instant>,
    media: MediaWatch,
    polling: bool,
    me: String,
    members: BTreeMap<String, keys::Member>,
    blocked: BTreeMap<String, Blocked>,
    tracks: BTreeMap<String, TrackInfo>,
    speaking: HashSet<String>,
    want_speaking: bool,
    speaking_sent: Option<bool>,
    speak_at: Instant,
    speak_due: Option<Instant>,
    decrypt_fail: HashMap<String, (u32, Instant)>,
    rekeyed: HashMap<String, Instant>,
    muted: bool,
    deafened: bool,
    game_muted: bool,
    game_injected: bool,
    trust_all: bool,
    ptt_held: bool,
    devices: Devices,
    share: Option<ShareHandle>,
    share_view: ShareView,
    share_apps: Vec<AppStream>,
    settings: CallSettings,
    settings_path: Option<PathBuf>,
    gate: Arc<MicGate>,
    mic: Arc<tokio::sync::Mutex<MicSlot>>,
    wish: Arc<parking_lot::Mutex<AudioWish>>,
    game_watch: Option<tokio::task::JoinHandle<()>>,
    dirty: bool,
    published_at: Instant,
    v: u64,
}

impl Actor {
    fn new(core: Arc<dyn CoreLink>, opts: Options, tx: UnboundedSender<Msg>, snap_out: Arc<Mutex<Snapshot>>, version: tokio::sync::watch::Sender<u64>, taps: Arc<Taps>) -> Actor {
        let mic: Arc<tokio::sync::Mutex<MicSlot>> = Default::default();
        let wish: Arc<parking_lot::Mutex<AudioWish>> = Arc::new(parking_lot::Mutex::new(AudioWish {
            on: false, processing: opts.settings.processing, mic_id: opts.settings.mic_id.clone(), out_id: opts.settings.out_id.clone(),
        }));
        let (m2, mode, w2) = (mic.clone(), opts.audio.clone(), wish.clone());
        let publish: Publish = Arc::new(move |on| {
            let (m, mode, w) = (m2.clone(), mode.clone(), w2.clone());
            Box::pin(async move { w.lock().on = on; realize_mic(m, mode, w).await })
        });
        let settings = opts.settings.clone();
        let settings_path = opts.settings_path.clone();
        Actor {
            core, lane: Lane::new("mega-call-core"), game_lane: Lane::new("mega-call-keys"), opts, tx, snap_out, version, taps,
            gid: None, room_name: String::new(), phase: "idle", error: None, events: VecDeque::new(), call: 0, gen: 0, session: None, token: None,
            attempt: 0, retry_at: None, rejoin_started: None, reconnecting_since: None, media: MediaWatch::default(), polling: false, me: String::new(),
            members: BTreeMap::new(), blocked: BTreeMap::new(), tracks: BTreeMap::new(), speaking: HashSet::new(), want_speaking: false, speaking_sent: None,
            speak_at: Instant::now() - Duration::from_secs(5), speak_due: None, decrypt_fail: HashMap::new(), rekeyed: HashMap::new(),
            muted: false, deafened: false, game_muted: false, game_injected: false, trust_all: false, ptt_held: false, devices: Devices::default(), share: None, share_view: ShareView { state: "idle".into(), ..Default::default() }, share_apps: vec![],
            settings, settings_path, gate: Arc::new(MicGate::new(publish)), mic, wish, game_watch: None, dirty: true, published_at: Instant::now(), v: 0,
        }
    }

    async fn run(mut self, mut rx: UnboundedReceiver<Msg>) {
        let mut tick = tokio::time::interval(Duration::from_millis(60));
        let mut poll = tokio::time::interval(rc::POLL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                m = rx.recv() => match m { Some(m) => self.on_msg(m), None => break },
                _ = tick.tick() => self.on_tick(),
                _ = poll.tick() => self.on_poll(),
            }
        }
        self.teardown();
    }

    fn log(&mut self, m: impl Into<String>) {
        let m = m.into();
        log::info!("call: {m}");
        self.events.push_back(m);
        while self.events.len() > 100 { self.events.pop_front(); }
        self.dirty = true;
    }
    fn fail(&mut self, e: CallError) { self.log(format!("error: {}", e.msg)); self.error = Some((e, Instant::now())); self.dirty = true; }

    // -------------------------------------------------------------------------------------------- messages
    fn on_msg(&mut self, m: Msg) {
        match m {
            Msg::Cmd(c) => self.on_cmd(c),
            Msg::Connected { gen, attempt, result } => self.on_connected(gen, attempt, result),
            Msg::Polled { call, result } => { if call == self.call { self.polling = false; self.on_polled(result); } }
            Msg::Game { call, news } => { if call == self.call { self.on_game(news); } }
            Msg::Mic { call, result } => { if call == self.call { self.on_mic(result); } }
            Msg::Devices { call, result } => { if call == self.call { self.on_devices(result); } }
            Msg::Device { call, result } => { if call == self.call { self.on_device(result); } }
            Msg::Plout { identity, ms, reply } => self.on_plout(identity, ms, reply),
            Msg::MicOptions { reply } => self.on_mic_options(reply),
            Msg::ShareStats { reply } => match &self.share { Some(h) => h.send(ShareCmd::Stats(reply)), None => { let _ = reply.send(json!({"layers": []})); } },
            Msg::Share(r) => self.on_share_report(r),
            Msg::ShareApps(r) => match r {
                Ok(v) => { self.log(format!("{} apps playing sound", v.len())); self.share_apps = v; self.dirty = true; }
                Err(e) => { self.log(format!("app list unavailable: {e}")); self.share_apps.clear(); self.dirty = true; }
            },
            Msg::Room { gen, ev } => { if self.session.as_ref().is_some_and(|s| s.gen == gen) { self.on_room(ev); } }
        }
    }

    fn on_cmd(&mut self, c: Command) {
        match c {
            Command::Join { gid, name } => self.join(gid, name),
            Command::Leave => { self.leave(); self.phase = "idle"; self.error = None; }
            Command::SetMuted(m) => self.set_muted(m),
            Command::SetDeafened(d) => {
                self.deafened = d;
                for t in self.tracks.values() { if let Some(a) = &t.audio { if d { a.disable() } else { a.enable() } } }
                self.announce_deafened();
                if d { self.set_muted(true); }
                self.dirty = true;
            }
            Command::SetVolume { account, volume } => {
                let v = volume.clamp(0., 10.);
                self.settings.volumes.insert(account.clone(), v);
                for t in self.tracks.values() { if self.members.get(&t.identity).is_some_and(|m| m.account == account) { if let Some(a) = &t.audio { a.rtc_track().set_volume(v); } } }
                self.save_settings();
                self.dirty = true;
            }
            Command::ListDevices => self.list_devices(),
            Command::SetMicDevice { id } => { self.settings.mic_id = Some(id.clone()); self.wish.lock().mic_id = Some(id); self.save_settings(); self.apply_audio(); }
            Command::SetOutDevice { id } => { self.settings.out_id = Some(id.clone()); self.wish.lock().out_id = Some(id); self.save_settings(); self.apply_audio(); }
            Command::SetProcessing(p) => { self.set_processing(p); }
            Command::SetPushToTalk(p) => { self.settings.push_to_talk = p; self.save_settings(); if p { self.ptt_held = false; self.set_muted(true); } self.dirty = true; }
            Command::PushToTalk(down) => self.push_to_talk(down),
            Command::StartShare { profile, source, audio } => self.share_cmd(ShareCmd::Start { profile, source, audio }, true),
            Command::StopShare => self.share_cmd(ShareCmd::Stop, false),
            Command::SwitchShareSource(s) => self.share_cmd(ShareCmd::Switch(s), true),
            Command::SetShareProfile(p) => self.share_cmd(ShareCmd::Profile(p), false),
            Command::ListShareApps => {
                let tx = self.tx.clone();
                tokio::task::spawn_blocking(move || { let _ = tx.send(Msg::ShareApps(super::appaudio::list())); });
            }
            Command::StartShareAudio { stream } => self.share_cmd(ShareCmd::AudioStart(stream), true),
            Command::StopShareAudio => self.share_cmd(ShareCmd::AudioStop, false),
            Command::TestShareEnd => self.share_cmd(ShareCmd::TestEnd, false),
            Command::TestGame(g) => match g {
                Some(m) => { self.game_injected = true; self.on_game_state(m, Some("test".into())); }
                None => { self.game_injected = false; self.on_game_state(false, None); }
            },
            Command::TestTrustAll(t) => { self.trust_all = t; self.review_all(); }
        }
    }

    /// The share task is made on first use. Starting anything needs a call (`needs_call`), as in call.js `startShare`.
    fn share_cmd(&mut self, c: ShareCmd, needs_call: bool) {
        if needs_call && self.session.is_none() { self.fail(CallError::new(Kind::Screen, "Join the call first.")); return; }
        if self.share.is_none() {
            let (tx, rt) = (self.tx.clone(), tokio::runtime::Handle::current());
            let h = share::spawn(&rt, Arc::new(move |r| { let _ = tx.send(Msg::Share(r)); }));
            if let Some(s) = &self.session { h.send(ShareCmd::Session(Some(Sess { room: s.room.clone(), book: s.book.clone() }))); }
            self.share = Some(h);
        }
        if let Some(h) = &self.share { h.send(c); }
    }

    fn on_share_report(&mut self, r: Report) {
        match r {
            Report::View(v) => { self.share_view = v; self.dirty = true; }
            Report::Log(m) => self.log(m),
            Report::Fail(m) => self.fail(CallError::new(Kind::Screen, m)),
        }
    }

    fn save_settings(&self) { if let Some(p) = &self.settings_path { self.settings.save(p); } }

    fn set_processing(&mut self, p: Processing) {
        // call.js `setNoiseSuppression`: turning noise suppression on also forces echo cancellation on.
        let p = if p.noise_suppression && !p.echo_cancellation { p.with_noise(true) } else { p };
        self.settings.processing = p;
        self.wish.lock().processing = p;
        self.save_settings();
        self.apply_audio();
    }

    fn push_to_talk(&mut self, down: bool) {
        if !self.settings.push_to_talk || self.deafened { return; }
        self.ptt_held = down;
        self.set_muted(!down);
        self.dirty = true;
    }

    fn apply_audio(&mut self) {
        if self.session.is_none() { self.dirty = true; return; }
        let (mic, mode, wish, tx, call) = (self.mic.clone(), self.opts.audio.clone(), self.wish.clone(), self.tx.clone(), self.call);
        tokio::spawn(async move { if let Err(e) = realize_mic(mic, mode, wish).await { let _ = tx.send(Msg::Device { call, result: Err(e) }); } });
        self.dirty = true;
    }

    fn list_devices(&mut self) {
        if matches!(self.opts.audio, AudioMode::Silent { .. }) { self.devices = Devices::default(); self.dirty = true; return; }
        let (tx, call) = (self.tx.clone(), self.call);
        tokio::task::spawn_blocking(move || { let r = audio::list_devices(); let _ = tx.send(Msg::Devices { call, result: r }); });
    }

    fn on_devices(&mut self, r: Result<Devices, String>) {
        match r {
            Ok(d) => { self.log(format!("{} microphones, {} speakers", d.mic.len(), d.out.len())); self.devices = d; }
            Err(e) => { self.log(format!("device list unavailable: {e}")); self.devices = Devices::default(); }
        }
        self.dirty = true;
    }

    fn on_device(&mut self, r: Result<(), String>) {
        if let Err(e) = r { self.log(format!("audio device: {e}")); self.fail(CallError::new(Kind::Device, e)); }
    }

    fn on_plout(&mut self, identity: String, ms: u64, reply: tokio::sync::oneshot::Sender<Value>) {
        let t = self.tracks.values().find(|t| t.identity == identity && t.source == TrackSource::Microphone).and_then(|t| t.audio.clone());
        tokio::spawn(async move {
            let v = match t {
                Some(t) => {
                    let e0 = audio::playout_energy(&t).await;
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    let e1 = audio::playout_energy(&t).await;
                    json!({"present": true, "energy": e1 - e0})
                }
                None => json!({"present": false}),
            };
            let _ = reply.send(v);
        });
    }

    fn on_mic_options(&mut self, reply: tokio::sync::oneshot::Sender<Value>) {
        let mic = self.mic.clone();
        tokio::spawn(async move {
            let m = mic.lock().await;
            let v = match m.publication.as_ref() {
                Some(p) => {
                    let sid = p.sid().to_string();
                    match p.track() {
                        Some(LocalTrack::Audio(a)) => {
                            let o = a.rtc_source().audio_options();
                            json!({"present": true, "sid": sid, "echoCancellation": o.echo_cancellation, "noiseSuppression": o.noise_suppression, "autoGainControl": o.auto_gain_control})
                        }
                        _ => json!({"present": true, "sid": sid}),
                    }
                }
                None => json!({"present": false}),
            };
            let _ = reply.send(v);
        });
    }

    fn set_muted(&mut self, m: bool) {
        self.muted = m;
        let (g, tx, call) = (self.gate.clone(), self.tx.clone(), self.call);
        tokio::spawn(async move { if let Err(e) = g.set_manual(m).await { let _ = tx.send(Msg::Mic { call, result: Err(e) }); } });
        self.announce_speaking(true);   // someone who is muted is not "speaking" to the rail, whatever the meter says
        self.dirty = true;
    }

    // -------------------------------------------------------------------------------------------- join and leave
    fn join(&mut self, gid: String, name: String) {
        if self.gid.is_some() { self.leave(); }
        self.reset();
        self.gid = Some(gid);
        self.room_name = name;
        self.phase = "connecting";
        self.muted = self.settings.push_to_talk || self.muted;
        self.start_attempt(Attempt::Join(0));
        self.start_game_watch();
    }

    fn reset(&mut self) {
        self.error = None; self.events.clear(); self.token = None; self.attempt = 0; self.retry_at = None; self.rejoin_started = None;
        self.reconnecting_since = None; self.media.reset(); self.members.clear(); self.blocked.clear(); self.tracks.clear();
        self.speaking.clear(); self.speaking_sent = None; self.speak_due = None; self.decrypt_fail.clear(); self.game_muted = false;
        self.polling = false; self.dirty = true;
    }

    fn leave(&mut self) {
        self.call += 1;
        if let Some(w) = self.game_watch.take() { w.abort(); }
        self.retry_at = None;
        self.rejoin_started = None;
        self.teardown();
        if let Some(h) = &self.share { h.send(ShareCmd::Stop); h.send(ShareCmd::AudioStop); }
        self.gid = None;
        self.reset();
        self.muted = false; self.deafened = false; self.ptt_held = false;
        self.dirty = true;
    }

    /// Ends the current session (not the call): mic, room, tracks.
    fn teardown(&mut self) {
        self.gate.close();
        if let Some(h) = &self.share { h.send(ShareCmd::Session(None)); }
        self.speak_due = None; self.speaking_sent = None;
        let mic = self.mic.clone();
        let old = self.session.take().map(|s| (s.room, s.gen));
        self.gen += 1;
        tokio::spawn(async move {
            let Some((room, gen)) = old else { return };
            {
                // only this session's microphone: a newer session may already own the slot
                let mut m = mic.lock().await;
                if m.gen == gen {
                    if let Some(f) = m.feeder.take() { f.abort(); }
                    m.publication = None; m.room = None; m.book = None;
                    // the microphone indicator goes off when the call ends, whatever the mute state was
                    if let Some(p) = &m.platform { let _ = p.stop_recording(); }
                }
            }
            let _ = tokio::time::timeout(Duration::from_secs(3), room.close()).await;
        });
        self.tracks.clear();
        self.speaking.clear();
        self.dirty = true;
    }

    fn start_attempt(&mut self, a: Attempt) {
        self.gen += 1;
        let gen = self.gen;
        let (core, lane, tx) = (self.core.clone(), self.lane.clone(), self.tx.clone());
        let gid = self.gid.clone().unwrap_or_default();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let retry = match a { Attempt::Join(i) | Attempt::Rejoin(i) => i > 0 };
        let reuse = self.token.clone().filter(|t| retry && rc::token_reusable(t.expires, now));
        let limit = match a { Attempt::Join(_) => rc::FIRST_JOIN_LIMIT, Attempt::Rejoin(i) => rc::attempt_limit(i) };
        tokio::spawn(async move {
            let result = match tokio::time::timeout(limit, connect(core, lane, gid, reuse)).await {
                Ok(r) => r,
                Err(_) => Err(CallError::new(Kind::Network, "connect: the call did not come up in time")),
            };
            let _ = tx.send(Msg::Connected { gen, attempt: a, result });
        });
    }

    fn on_connected(&mut self, gen: u64, attempt: Attempt, result: Result<Conn, CallError>) {
        if gen != self.gen || self.gid.is_none() {
            // left, or another attempt took over, while this one was under way
            if let Ok(c) = result { tokio::spawn(async move { let _ = tokio::time::timeout(Duration::from_secs(3), c.room.close()).await; }); }
            return;
        }
        match result {
            Ok(conn) => {
                if let Attempt::Rejoin(_) = attempt { self.log("rejoined automatically"); self.error = None; }
                self.rejoin_started = None;
                self.attempt = 0;
                self.established(conn);
            }
            Err(e) => {
                if token_refused(&e.msg) { self.token = None; }
                match attempt {
                    Attempt::Join(i) => {
                        if e.kind == Kind::Network && i < rc::JOIN_RETRY.len() {
                            self.attempt = i as u32 + 1;
                            self.log(format!("could not connect (attempt {}), trying again: {}", i + 1, e.msg));
                            self.retry_at = Some((Instant::now() + rc::JOIN_RETRY[i], Attempt::Join(i + 1)));
                        } else {
                            self.leave();
                            self.phase = "error";
                            self.fail(e);
                        }
                    }
                    Attempt::Rejoin(i) => {
                        self.log(format!("rejoin attempt {} failed: {}", i + 1, e.msg));
                        if matches!(e.kind, Kind::NotMember | Kind::Removed | Kind::Unsupported) || e.msg.contains("no longer a member") {
                            let e = if e.kind == Kind::NotMember { CallError::new(Kind::Removed, "You were removed from this room, so the call was ended on this device.") } else { e };
                            self.leave(); self.phase = "error"; self.fail(e);
                            return;
                        }
                        let since = self.rejoin_started.map(|t| t.elapsed()).unwrap_or_default();
                        if since > rc::REJOIN_GIVE_UP {
                            self.leave(); self.phase = "error";
                            self.fail(CallError::new(Kind::Network, format!("Could not get back into the call after 10 minutes. {}", e.msg)));
                            return;
                        }
                        self.attempt = i as u32 + 1;
                        self.error = Some((CallError::new(Kind::Network, format!("Connection lost; trying again (attempt {})…", i + 1)), Instant::now()));
                        self.retry_at = Some((Instant::now() + rc::rejoin_delay(i, since), Attempt::Rejoin(i + 1)));
                    }
                }
            }
        }
        self.dirty = true;
    }

    fn established(&mut self, conn: Conn) {
        let Conn { room, mut events, book, st, tok } = conn;
        if !room.e2ee_manager().enabled() {
            tokio::spawn(async move { let _ = room.close().await; });
            self.leave(); self.phase = "error";
            self.fail(CallError::new(Kind::NoE2ee, "Frame encryption could not be switched on, so the call was not started."));
            return;
        }
        let gen = self.gen;
        self.token = Some(tok);
        self.me = st.me.clone();
        self.members = st.members.iter().map(|m| (m.device.clone(), m.clone())).collect();
        let book = Arc::new(Mutex::new(book));
        self.session = Some(Session { room: room.clone(), book: book.clone(), gen });
        if let Some(h) = &self.share { h.send(ShareCmd::Session(Some(Sess { room: room.clone(), book: book.clone() }))); }
        self.phase = "connected";
        self.reconnecting_since = None;
        self.media.reset();
        self.log(format!("connected, epoch {}", st.epoch));
        let tx = self.tx.clone();
        tokio::spawn(async move { while let Some(ev) = events.recv().await { if tx.send(Msg::Room { gen, ev }).is_err() { break; } } });
        if self.deafened { self.announce_deafened(); }
        self.announce_speaking(true);
        // hearing the others must not wait for our microphone: review and subscribe first
        self.review_all();
        self.gate.preset(self.muted, self.game_muted);
        let (mic, gate, tx, call) = (self.mic.clone(), self.gate.clone(), self.tx.clone(), self.call);
        tokio::spawn(async move {
            { let mut m = mic.lock().await; if let Some(f) = m.feeder.take() { f.abort(); } m.gen = gen; m.room = Some(room); m.book = Some(book); m.publication = None; }
            let r = gate.open().await;
            let _ = tx.send(Msg::Mic { call, result: r });
        });
        self.dirty = true;
    }

    fn on_mic(&mut self, r: Result<(), String>) {
        match r {
            Ok(()) => self.review_all(),
            Err(e) => {
                self.log(format!("microphone unavailable: {e}"));
                self.muted = true;
                self.gate.preset(true, self.game_muted);
                let kind = if e.starts_with("no-e2ee") { Kind::NoE2ee } else { Kind::Mic };
                let msg = e.trim_start_matches("no-e2ee: ").to_string();
                self.fail(CallError::new(kind, format!("{msg} You are in the call muted and can still listen.")));
            }
        }
    }

    /// The session is gone for good, or the SDK's resume is taking too long: a fresh room, keys and token.
    fn auto_rejoin(&mut self, why: &str) {
        if self.rejoin_started.is_some() || self.gid.is_none() { return; }
        self.log(format!("rebuilding the call: {why}"));
        self.rejoin_started = Some(Instant::now());
        self.phase = "reconnecting";
        self.teardown();
        self.blocked.clear();
        self.start_attempt(Attempt::Rejoin(0));
    }

    // -------------------------------------------------------------------------------------------- heartbeat and keys
    fn on_poll(&mut self) {
        let Some(gid) = self.gid.clone() else { return };
        if self.polling { return; }
        if let Some(s) = &self.session {
            if self.rejoin_started.is_none() && self.reconnecting_since.is_none() && self.media.poll(s.room.connection_state() == ConnectionState::Connected) {
                self.auto_rejoin("media path down");
                return;
            }
        }
        self.poll_now(gid);
    }
    fn poll_now(&mut self, gid: String) {
        self.polling = true;
        let (core, lane, tx, call) = (self.core.clone(), self.lane.clone(), self.tx.clone(), self.call);
        tokio::spawn(async move {
            let r = match tokio::time::timeout(rc::RPC_TIMEOUT, lane.call(&core, "call_state", json!({"gid": gid}))).await { Ok(r) => r, Err(_) => Err("the app did not answer in time".into()) };
            let _ = tx.send(Msg::Polled { call, result: r });
        });
    }

    fn on_polled(&mut self, r: Result<Value, String>) {
        match r {
            Ok(v) => {
                let Ok(st) = keys::CallState::parse(&v) else { return };
                let Some(s) = &self.session else { self.members = st.members.iter().map(|m| (m.device.clone(), m.clone())).collect(); return };
                let (room, book) = (s.room.clone(), s.book.clone());
                let mut b = book.lock();
                if b.epoch != Some(st.epoch) || st.members.len() != self.members.len() {
                    let prev = b.epoch;
                    let a = b.apply(&st, Instant::now());
                    b.point_senders(room.e2ee_manager());
                    drop(b);
                    self.members = st.members.iter().map(|m| (m.device.clone(), m.clone())).collect();
                    if a.rotated { self.log(format!("keys rotated: epoch {} to {}", prev.unwrap_or(0), st.epoch)); }
                    self.review_all();
                    self.dirty = true;
                }
            }
            Err(m) => {
                if removed_from_room(&m) {
                    let e = removed_notice(&m);
                    self.leave(); self.phase = "error"; self.fail(e);
                }
            }
        }
    }

    // -------------------------------------------------------------------------------------------- remote tracks
    fn review_all(&mut self) {
        let Some(s) = &self.session else { return };
        let room = s.room.clone();
        for (_, rp) in room.remote_participants() { self.review_participant(&rp); }
        self.dirty = true;
    }
    fn review_participant(&mut self, rp: &RemoteParticipant) { for (_, p) in rp.track_publications() { self.review_pub(rp, &p); } }
    fn review_pub(&mut self, rp: &RemoteParticipant, p: &RemoteTrackPublication) {
        let id = rp.identity().to_string();
        let key = format!("{id}/{}", p.sid());
        match review::verdict(self.members.contains_key(&id), p.encryption_type() != EncryptionType::None, self.trust_all) {
            Some(why) => {
                if p.is_subscribed() || p.is_desired() { p.set_subscribed(false); }
                if !self.blocked.contains_key(&key) { self.log(format!("blocked {:?} from {}: {why}", p.source(), &id[..id.len().min(8)])); }
                self.blocked.insert(key, Blocked { identity: id, source: format!("{:?}", p.source()), why: why.into() });
            }
            None => {
                self.blocked.remove(&key);
                if !p.is_subscribed() && !p.is_desired() { p.set_subscribed(true); }
            }
        }
        self.dirty = true;
    }

    fn rekey(&mut self, identity: &str) { if let Some(s) = &self.session { s.book.lock().rekey(identity); } }

    fn on_room(&mut self, ev: RoomEvent) {
        match ev {
            RoomEvent::ParticipantConnected(rp) => { self.rekey(rp.identity().as_str()); self.review_participant(&rp); }
            RoomEvent::ParticipantDisconnected(rp) => {
                let id = rp.identity().to_string();
                self.speaking.remove(&id);
                self.tracks.retain(|k, _| !k.starts_with(&format!("{id}/")));
                self.dirty = true;
            }
            RoomEvent::TrackPublished { publication, participant } => self.review_pub(&participant, &publication),
            RoomEvent::TrackSubscribed { track, publication, participant } => {
                let id = participant.identity().to_string();
                // second line of defence: checked again before anything is attached
                if review::verdict(self.members.contains_key(&id), publication.encryption_type() != EncryptionType::None, self.trust_all).is_some() { publication.set_subscribed(false); return; }
                self.rekey(&id);
                let key = format!("{id}/{}", publication.sid());
                let audio = match track {
                    RemoteTrack::Audio(a) => {
                        if self.deafened { a.disable(); }
                        if let Some(v) = self.members.get(&id).and_then(|m| self.settings.volumes.get(&m.account)) { a.rtc_track().set_volume(*v); }
                        if let AudioMode::Silent { .. } = self.opts.audio {
                            let (taps, who, t) = (self.taps.clone(), id.clone(), a.clone());
                            tokio::spawn(async move { let mut s = NativeAudioStream::new(t.rtc_track(), 48000, 1); while let Some(f) = s.next().await { taps.push(&who, &f); } });
                        }
                        Some(a)
                    }
                    RemoteTrack::Video(_) => None,
                };
                self.tracks.insert(key, TrackInfo { identity: id, source: publication.source(), audio });
                self.dirty = true;
            }
            RoomEvent::TrackUnsubscribed { publication, participant, .. } => { self.tracks.remove(&format!("{}/{}", participant.identity(), publication.sid())); self.dirty = true; }
            RoomEvent::TrackUnpublished { publication, participant } => {
                let k = format!("{}/{}", participant.identity(), publication.sid());
                self.tracks.remove(&k); self.blocked.remove(&k); self.dirty = true;
            }
            RoomEvent::LocalTrackPublished { publication, .. } => {
                if publication.encryption_type() == EncryptionType::None {
                    self.log("local track not encrypted: unpublishing");
                    if let Some(s) = &self.session { let (r, sid) = (s.room.clone(), publication.sid()); tokio::spawn(async move { let _ = r.local_participant().unpublish_track(&sid).await; }); }
                    self.fail(CallError::new(Kind::NoE2ee, "A track of yours was not encrypted and was stopped."));
                }
                self.point_senders();
            }
            RoomEvent::LocalTrackRepublished { .. } => { self.point_senders(); self.log("track republished by the SDK"); }
            RoomEvent::ActiveSpeakersChanged { speakers } => {
                self.speaking = speakers.iter().map(|p| p.identity().to_string()).collect();
                self.announce_speaking(false);
                self.dirty = true;
            }
            RoomEvent::E2eeStateChanged { participant, state } => {
                use livekit::webrtc::native::frame_cryptor::EncryptionState as S;
                let id = participant.identity().to_string();
                if id != self.me && matches!(state, S::DecryptionFailed | S::MissingKey | S::InternalError) {
                    let e = self.decrypt_fail.entry(id.clone()).or_insert((0, Instant::now()));
                    e.0 += 1; e.1 = Instant::now();
                    // self-heal: install this member's key again, at most every 2 s
                    if self.rekeyed.get(&id).is_none_or(|t| t.elapsed() > Duration::from_secs(2)) { self.rekeyed.insert(id.clone(), Instant::now()); self.rekey(&id); }
                    self.dirty = true;
                }
            }
            RoomEvent::Reconnecting => { self.phase = "reconnecting"; self.reconnecting_since = Some(Instant::now()); self.log("connection lost; reconnecting"); }
            RoomEvent::Reconnected => {
                if self.rejoin_started.is_none() {
                    self.phase = "connected"; self.reconnecting_since = None; self.log("reconnected");
                    self.point_senders();
                    if let Some(g) = self.gid.clone() { if !self.polling { self.poll_now(g); } }
                }
            }
            RoomEvent::Disconnected { reason } => { self.log(format!("disconnected: {reason:?}")); self.auto_rejoin("the session ended"); }
            RoomEvent::TrackMuted { .. } | RoomEvent::TrackUnmuted { .. } | RoomEvent::ParticipantAttributesChanged { .. } => self.dirty = true,
            _ => {}
        }
    }

    fn point_senders(&mut self) { if let Some(s) = &self.session { s.book.lock().point_senders(s.room.e2ee_manager()); } }

    // -------------------------------------------------------------------------------------------- speaking, deafened, game keys
    fn announce_deafened(&mut self) {
        let Some(s) = &self.session else { return };
        let (lp, v) = (s.room.local_participant(), if self.deafened { "1" } else { "0" });
        tokio::spawn(async move { let _ = lp.set_attributes(HashMap::from([("deaf".to_string(), v.to_string())])).await; });
    }
    /// At most one change per 600 ms; a mute change goes at once.
    fn announce_speaking(&mut self, now: bool) {
        if self.session.is_none() || self.me.is_empty() { return; }
        self.want_speaking = self.speaking.contains(&self.me) && !self.muted && !self.game_muted;
        if Some(self.want_speaking) == self.speaking_sent { return; }
        let wait = Duration::from_millis(600).saturating_sub(self.speak_at.elapsed());
        if now || wait.is_zero() { self.send_speaking(); } else if self.speak_due.is_none() { self.speak_due = Some(Instant::now() + wait); }
    }
    fn send_speaking(&mut self) {
        self.speak_due = None;
        if Some(self.want_speaking) == self.speaking_sent { return; }
        let Some(s) = &self.session else { return };
        self.speaking_sent = Some(self.want_speaking);
        self.speak_at = Instant::now();
        let (lp, v) = (s.room.local_participant(), if self.want_speaking { "1" } else { "0" });
        tokio::spawn(async move { let _ = lp.set_attributes(HashMap::from([("speaking".to_string(), v.to_string())])).await; });
    }

    /// Asks the core about game keys for as long as the call lasts. It answers at once when something changes and every
    /// few seconds regardless, so 9 s of silence means it is gone, and then the keys hold nothing.
    fn start_game_watch(&mut self) {
        if let Some(w) = self.game_watch.take() { w.abort(); }
        let (core, lane, tx, call, gid) = (self.core.clone(), self.game_lane.clone(), self.tx.clone(), self.call, self.gid.clone().unwrap_or_default());
        self.game_watch = Some(tokio::spawn(async move {
            let mut seq = 0u64;
            loop {
                match tokio::time::timeout(Duration::from_secs(9), lane.call(&core, "game_mute_wait", json!({"gid": gid, "since": seq}))).await {
                    Ok(Ok(v)) => {
                        seq = v["seq"].as_u64().unwrap_or(seq);
                        let news = GameNews::State { seq, muted: v["muted"].as_bool().unwrap_or(false), game: v["game"].as_str().map(String::from) };
                        if tx.send(Msg::Game { call, news }).is_err() { return; }
                    }
                    other => {
                        let m = match other { Ok(Err(e)) => e, _ => "the app stopped answering".into() };
                        let slow = m.contains("unknown method") || m.contains("not allowed");
                        if tx.send(Msg::Game { call, news: GameNews::Lost(m) }).is_err() { return; }
                        tokio::time::sleep(Duration::from_secs(if slow { 30 } else { 1 })).await;
                    }
                }
            }
        }));
    }
    fn on_game(&mut self, n: GameNews) {
        if self.game_injected { return; }
        match n {
            GameNews::State { muted, game, .. } => self.on_game_state(muted, game),
            GameNews::Lost(m) => { if self.game_muted { self.log(format!("game keys lost: {m}")); } self.on_game_state(false, None); }
        }
    }
    fn on_game_state(&mut self, muted: bool, _game: Option<String>) {
        if muted == self.game_muted { return; }
        self.game_muted = muted;
        self.log(if muted { "muted by game key" } else { "game key let go" });
        let (g, tx, call) = (self.gate.clone(), self.tx.clone(), self.call);
        tokio::spawn(async move { if let Err(e) = g.set_game(muted).await { let _ = tx.send(Msg::Mic { call, result: Err(e) }); } });
        self.announce_speaking(true);
        self.dirty = true;
    }

    // -------------------------------------------------------------------------------------------- the clock
    fn on_tick(&mut self) {
        let now = Instant::now();
        if let Some(s) = &self.session {
            let mut b = s.book.lock();
            if b.tick(now) {
                let idx = b.index;
                b.point_senders(s.room.e2ee_manager());
                drop(b);
                self.log(format!("old key slot wiped, now on index {idx}"));
            }
        }
        if let Some(t) = self.reconnecting_since { if now.duration_since(t) > rc::RESUME_GRACE && self.rejoin_started.is_none() { self.auto_rejoin("the resume took too long"); } }
        if let Some((at, a)) = self.retry_at { if now >= at { self.retry_at = None; self.start_attempt(a); } }
        if self.speak_due.is_some_and(|t| now >= t) { self.send_speaking(); }
        if let Some((e, at)) = &self.error { if e.kind.passing() && at.elapsed() > PASSING_FADE { self.error = None; self.dirty = true; } }
        if self.dirty && self.published_at.elapsed() >= Duration::from_millis(60) { self.publish(); }
    }

    fn publish(&mut self) {
        self.dirty = false;
        self.published_at = Instant::now();
        let s = self.build_snapshot();
        let changed = { let mut g = self.snap_out.lock(); if *g != s { *g = s; true } else { false } };
        if changed { self.v += 1; let _ = self.version.send(self.v); }
    }

    fn build_snapshot(&self) -> Snapshot {
        let mut peers = vec![];
        let mut enc = Encryption::default();
        if let Some(s) = &self.session {
            let me_m = self.members.get(&self.me);
            peers.push(Peer { identity: self.me.clone(), me: true, name: me_m.map(|m| m.name.clone()).unwrap_or_else(|| "You".into()), account: me_m.map(|m| m.account.clone()), member: me_m.is_some(),
                speaking: self.speaking.contains(&self.me) && !self.muted && !self.game_muted, muted: self.muted || self.game_muted, deafened: self.deafened, volume: 1., cant_decrypt: false });
            for (id, rp) in s.room.remote_participants() {
                let id = id.to_string();
                let m = self.members.get(&id);
                let mic = rp.track_publications().values().find(|p| p.source() == TrackSource::Microphone).map(|p| p.is_muted());
                peers.push(Peer {
                    identity: id.clone(), me: false, name: m.map(|m| m.name.clone()).unwrap_or_else(|| format!("Unknown device {}", &id[..id.len().min(8)])),
                    account: m.map(|m| m.account.clone()), member: m.is_some(), speaking: self.speaking.contains(&id), muted: mic.unwrap_or(true),
                    deafened: rp.attributes().get("deaf").is_some_and(|v| v == "1"), volume: m.and_then(|m| self.settings.volumes.get(&m.account)).copied().unwrap_or(1.),
                    cant_decrypt: self.decrypt_fail.get(&id).is_some_and(|(_, t)| t.elapsed() < Duration::from_secs(6)),
                });
            }
            let b = s.book.lock();
            enc = Encryption { active: s.room.e2ee_manager().enabled(), epoch: b.epoch, key_index: b.epoch.map(|_| b.index), rotations: b.rotations, devices: b.devices(), wipes: b.wipes_done,
                sender_index: s.room.e2ee_manager().frame_cryptors().iter().filter(|((id, _), _)| id.as_str() == self.me).map(|(_, fc)| fc.key_index()).collect() };
        }
        Snapshot {
            phase: self.phase.into(), error: self.error.as_ref().map(|(e, _)| ErrorView { kind: e.kind.as_str().into(), msg: e.msg.clone() }), gid: self.gid.clone(), room_name: self.room_name.clone(),
            muted: self.muted, game_muted: self.game_muted, deafened: self.deafened, peers, encryption: enc, blocked: self.blocked.values().cloned().collect(),
            subscribed: self.tracks.iter().map(|(k, t)| format!("{}:{:?}", k, t.source)).collect(), attempt: self.attempt,
            events: self.events.iter().rev().take(12).rev().cloned().collect(),
            devices: self.devices.clone(), mic_id: self.settings.mic_id.clone(), out_id: self.settings.out_id.clone(),
            echo_cancellation: self.settings.processing.echo_cancellation, noise_suppression: self.settings.processing.noise_suppression,
            auto_gain_control: self.settings.processing.auto_gain_control, push_to_talk: self.settings.push_to_talk, ptt_held: self.ptt_held,
            share: self.share_view.clone(), share_apps: self.share_apps.clone(),
        }
    }
}

/// One attempt: credentials (keys and a token, together), the keys in place, then the room. Nothing is published here.
async fn connect(core: Arc<dyn CoreLink>, lane: Lane, gid: String, reuse: Option<Token>) -> Result<Conn, CallError> {
    let st = lane.call(&core, "call_state", json!({"gid": gid})).await.map_err(|m| if removed_from_room(&m) { removed_notice(&m) } else { explain(&m) })?;
    let tok = match reuse { Some(t) => t, None => Token::parse(&lane.call(&core, "call_token", json!({"gid": gid})).await.map_err(|m| explain(&m))?)? };
    let st = keys::CallState::parse(&st).map_err(|e| CallError::new(Kind::Other, e))?;
    let mut book = KeyBook::new(keys::provider());
    book.apply(&st, Instant::now());   // keys are in place before the room can carry any media
    let mut o = RoomOptions::default();
    o.auto_subscribe = false;
    o.adaptive_stream = false;
    o.dynacast = true;
    o.join_retries = 0;   // retries are ours, with call.js's waits
    o.encryption = Some(E2eeOptions { encryption_type: EncryptionType::Gcm, key_provider: book.kp.clone() });
    o.ice_servers_filter = Some(ice::filter(tok.turn_port, None));
    let (room, events) = Room::connect(&tok.url, &tok.token, o).await.map_err(|e| {
        let m = e.to_string();
        let mut ce = explain(&format!("connect: {m}"));
        if token_refused(&m) { ce.msg = format!("{} (token refused: 401)", ce.msg); }
        ce
    })?;
    Ok(Conn { room: Arc::new(room), events, book, st, tok })
}
