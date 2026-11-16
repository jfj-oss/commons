//! A headless Rust LiveKit participant with mega's key scheme, for the encryption interop test (CALLS-PLAN.md step 0,
//! tools/gpui-call-interop.mjs). Not product code: it shares the product's key, ICE and tone modules so the test
//! checks the rules the app uses, and nothing else.
//!
//! Silent: the "microphone" is a sine pushed into a `NativeAudioSource`, hearing is measured on `NativeAudioStream`s,
//! and no audio device is opened (libwebrtc keeps its synthetic device).
//!
//! Protocol: the first line on stdin is the setup, every later line a command; every answer is one line of JSON on
//! stdout. Keys and tokens are never printed.
//!   setup   {"url","token","turn_port","state":<call_state>,"tone_hz":900,"controls":{"bad_key","index0","pbkdf2"}}
//!   {"cmd":"keys","state":<call_state>}              apply a newer call_state (rotation rules of keys.rs)
//!   {"cmd":"measure","ms":2500,"listen":{"<identity>":hz}}  tone share of each listed sender's microphone, video frames
//!   {"cmd":"holds","identity","index","key":<base64>}  does the key provider hold that key there
//!   {"cmd":"share","w":2560,"h":1440}                 publish a screen-share-shaped track (full size + one 540p layer)
//!   {"cmd":"unshare"}, {"cmd":"status"}, {"cmd":"leave"}
#[path = "../src/calls/ice.rs"] mod ice;
#[path = "../src/calls/keys.rs"] mod keys;
#[path = "../src/calls/tone.rs"] mod tone;

use futures::StreamExt;
use livekit::e2ee::key_provider::KeyDerivationAlgorithm;
use livekit::e2ee::{E2eeOptions, EncryptionType};
use livekit::options::{TrackPublishOptions, VideoCodec, VideoEncoding, VideoPreset};
use livekit::prelude::*;
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::{native::NativeAudioSource, AudioSourceOptions, RtcAudioSource};
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use livekit::webrtc::prelude::DegradationPreference;
use livekit::webrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
use livekit::webrtc::video_source::{native::NativeVideoSource, RtcVideoSource, VideoResolution};
use livekit::webrtc::video_stream::native::NativeVideoStream;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncBufReadExt;

#[derive(Default)]
struct Video { frames: u64, w: u32, h: u32 }
#[derive(Default)]
struct Shared {
    meters: BTreeMap<String, tone::ToneMeter>,
    video: BTreeMap<String, Video>,
    events: Vec<String>,
    e2ee: BTreeMap<String, String>,
    ice_seen: Vec<String>,
    republished_index: Vec<i32>,
    audio_tracks: BTreeMap<String, RemoteAudioTrack>,
}

/// The receiver's own playout measure (inbound-rtp `totalAudioEnergy`), which libwebrtc takes in the playout path.
async fn playout_energy(t: &RemoteAudioTrack) -> f64 {
    t.get_stats().await.unwrap_or_default().into_iter().find_map(|s| match s { livekit::webrtc::stats::RtcStats::InboundRtp(r) => Some(r.inbound.total_audio_energy), _ => None }).unwrap_or(-1.)
}

fn out(v: Value) { println!("{v}"); }
/// Answers carry the command's `id`, so a long `measure` does not hold up the commands sent after it.
fn reply(id: &Value, mut v: Value) { v["id"] = id.clone(); out(v); }
fn ts() -> i64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as i64 }

/// A flat colour with a small moving block, so the encoder always has something new to send.
fn frame(w: u32, h: u32, n: u64, yuv: (u8, u8, u8)) -> VideoFrame<I420Buffer> {
    let mut b = I420Buffer::new(w, h);
    let (sy, _, _) = b.strides();
    {
        let (y, u, v) = b.data_mut();
        y.fill(yuv.0); u.fill(yuv.1); v.fill(yuv.2);
        let (bx, bw) = ((n * 8 % (w as u64 - 64)) as usize, 32usize);
        for row in 0..bw.min(h as usize) { let o = row * sy as usize + bx; y[o..o + bw].fill(235); }
    }
    let mut f = VideoFrame::new(VideoRotation::VideoRotation0, b);
    f.timestamp_us = ts();
    f
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let setup: Value = serde_json::from_str(&lines.next_line().await.unwrap().expect("setup line")).expect("setup json");
    let ctl = |k: &str| setup["controls"][k].as_bool().unwrap_or(false);
    let (bad_key, index0, pbkdf2) = (ctl("bad_key"), ctl("index0"), ctl("pbkdf2"));
    let tone_hz = setup["tone_hz"].as_f64().unwrap_or(900.);
    let mut st = keys::CallState::parse(&setup["state"]).expect("call_state");
    if bad_key { st.keys.get_mut(&st.me).unwrap()[0] ^= 0xff; }   // control: our own key wrong by one byte
    let me = st.me.clone();
    let kdf = if pbkdf2 { KeyDerivationAlgorithm::PBKDF2 } else { KeyDerivationAlgorithm::HKDF };
    let book = Arc::new(Mutex::new(keys::KeyBook::new(livekit::e2ee::key_provider::KeyProvider::new(keys::options(kdf)))));
    book.lock().apply(&st, Instant::now());
    let shared = Arc::new(Mutex::new(Shared::default()));

    let mut opts = RoomOptions::default();
    opts.auto_subscribe = true;
    opts.adaptive_stream = false;
    opts.dynacast = true;
    opts.encryption = Some(E2eeOptions { encryption_type: EncryptionType::Gcm, key_provider: book.lock().kp.clone() });
    let sh = shared.clone();
    opts.ice_servers_filter = Some(ice::filter(setup["turn_port"].as_u64().map(|p| p as u16), Some(Arc::new(move |seen| sh.lock().ice_seen = seen))));
    let (room, mut events) = match Room::connect(setup["url"].as_str().unwrap(), setup["token"].as_str().unwrap(), opts).await {
        Ok(r) => r,
        Err(e) => { out(json!({"ready": false, "error": e.to_string()})); std::process::exit(1); }
    };
    let room = Arc::new(room);

    // microphone and camera, published before any media is fed to them
    let asrc = NativeAudioSource::new(AudioSourceOptions::default(), 48000, 1, 100);
    let atrack = LocalAudioTrack::create_audio_track("mic", RtcAudioSource::Native(asrc.clone()));
    let apub = room.local_participant().publish_track(LocalTrack::Audio(atrack), TrackPublishOptions { source: TrackSource::Microphone, dtx: true, ..Default::default() }).await.expect("publish audio");
    let vsrc = NativeVideoSource::new(VideoResolution { width: 640, height: 360 }, false);
    let vtrack = LocalVideoTrack::create_video_track("camera", RtcVideoSource::Native(vsrc.clone()));
    let vpub = room.local_participant().publish_track(LocalTrack::Video(vtrack), TrackPublishOptions { source: TrackSource::Camera, video_codec: VideoCodec::VP8, simulcast: false, ..Default::default() }).await.expect("publish video");
    // patch L2: the sender cryptors were born with the provider's latest index, nobody has touched them yet
    let born: Vec<i32> = room.e2ee_manager().frame_cryptors().iter().filter(|((id, _), _)| id.as_str() == me).map(|(_, fc)| fc.key_index()).collect();
    if index0 {
        // control: the sending index the SDK would have left without patch L2 and without rule 3
        for ((id, _), fc) in room.e2ee_manager().frame_cryptors() { if id.as_str() == me { fc.set_key_index(0); } }
    } else {
        book.lock().point_senders(room.e2ee_manager());
    }
    out(json!({"ready": true, "me": me, "epoch": st.epoch, "index": keys::index_of(st.epoch), "sender_index_at_birth": born,
        "audio_encrypted": apub.encryption_type() == EncryptionType::Gcm, "video_encrypted": vpub.encryption_type() == EncryptionType::Gcm,
        "ice_seen": shared.lock().ice_seen.clone(), "sid": room.local_participant().sid().to_string()}));

    tokio::spawn(async move {
        let mut t = tokio::time::interval(Duration::from_millis(10));
        let mut n = 0u64;
        loop {
            t.tick().await;
            let data = tone::sine(tone_hz, &mut n, 12000.);
            let _ = asrc.capture_frame(&AudioFrame { data: data.into(), sample_rate: 48000, num_channels: 1, samples_per_channel: 480 }).await;
        }
    });
    tokio::spawn(async move {
        let mut t = tokio::time::interval(Duration::from_millis(66));
        let mut n = 0;
        loop { t.tick().await; vsrc.capture_frame(&frame(640, 360, n, (127, 96, 67))); n += 1; }   // about rgb(32, 192, 64)
    });

    // room events
    let (sh, bk, rm, me2) = (shared.clone(), book.clone(), room.clone(), me.clone());
    tokio::spawn(async move {
        while let Some(ev) = events.recv().await {
            match ev {
                RoomEvent::TrackSubscribed { track, publication, participant } => {
                    let key = format!("{}/{:?}", participant.identity(), publication.source());
                    bk.lock().rekey(participant.identity().as_str());
                    sh.lock().events.push(format!("subscribed {} enc={:?}", key, publication.encryption_type()));
                    let sh = sh.clone();
                    match track {
                        RemoteTrack::Audio(a) => { sh.lock().audio_tracks.insert(key.clone(), a.clone()); tokio::spawn(async move {
                            let mut s = NativeAudioStream::new(a.rtc_track(), 48000, 1);
                            while let Some(f) = s.next().await { for (k, m) in sh.lock().meters.iter_mut() { if k.split('#').next() == Some(key.as_str()) { m.push(&f.data, f.sample_rate, f.num_channels); } } }
                        }); }
                        RemoteTrack::Video(v) => { tokio::spawn(async move {
                            let mut s = NativeVideoStream::new(v.rtc_track());
                            while let Some(f) = s.next().await { let mut g = sh.lock(); let r = g.video.entry(key.clone()).or_default(); r.frames += 1; r.w = f.buffer.width(); r.h = f.buffer.height(); }
                        }); }
                    }
                }
                RoomEvent::ParticipantConnected(p) => bk.lock().rekey(p.identity().as_str()),
                RoomEvent::E2eeStateChanged { participant, state } => { sh.lock().e2ee.insert(participant.identity().to_string(), format!("{state:?}")); }
                RoomEvent::LocalTrackRepublished { .. } => {
                    let idx: Vec<i32> = rm.e2ee_manager().frame_cryptors().iter().filter(|((id, _), _)| id.as_str() == me2).map(|(_, fc)| fc.key_index()).collect();
                    let mut g = sh.lock(); g.republished_index.extend(idx); g.events.push("republished".into());
                    if !index0 { bk.lock().point_senders(rm.e2ee_manager()); }
                }
                RoomEvent::Reconnecting => sh.lock().events.push("reconnecting".into()),
                RoomEvent::Reconnected => { sh.lock().events.push("reconnected".into()); if !index0 { bk.lock().point_senders(rm.e2ee_manager()); } }
                RoomEvent::Disconnected { reason } => sh.lock().events.push(format!("disconnected {reason:?}")),
                _ => {}
            }
        }
    });
    // the old key slot is wiped once its grace has passed
    let (bk, rm, sh) = (book.clone(), room.clone(), shared.clone());
    tokio::spawn(async move {
        let mut t = tokio::time::interval(Duration::from_millis(200));
        loop { t.tick().await; let mut b = bk.lock(); if b.tick(Instant::now()) { sh.lock().events.push(format!("old slot wiped, index {}", b.index)); if !index0 { b.point_senders(rm.e2ee_manager()); } } }
    });

    let mut share: Option<(LocalTrackPublication, tokio::task::JoinHandle<()>)> = None;
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(c) = serde_json::from_str::<Value>(&line) else { out(json!({"error": "bad json"})); continue };
        let id = c["id"].clone();
        let out = |v: Value| reply(&id, v);
        match c["cmd"].as_str().unwrap_or("") {
            "keys" => match keys::CallState::parse(&c["state"]) {
                Ok(st) => {
                    let mut b = book.lock();
                    let a = b.apply(&st, Instant::now());
                    let moved = if index0 { 0 } else { b.point_senders(room.e2ee_manager()) };
                    out(json!({"ok": true, "rotated": a.rotated, "epoch": st.epoch, "index": b.index, "senders_moved": moved}));
                }
                Err(e) => out(json!({"ok": false, "error": e})),
            },
            "measure" => {
                let listen = c["listen"].as_object().cloned().unwrap_or_default();
                let ms = c["ms"].as_u64().unwrap_or(2500);
                let (sh, id) = (shared.clone(), id.clone());
                tokio::spawn(async move {
                    // meters of their own for this window, so measures may overlap
                    let tag = format!("#{}", id);
                    {
                        let mut g = sh.lock();
                        for (who, hz) in &listen { g.meters.insert(format!("{who}/Microphone{tag}"), tone::ToneMeter::new(hz.as_f64().unwrap_or(500.))); }
                    }
                    let v0: BTreeMap<String, u64> = sh.lock().video.iter().map(|(k, v)| (k.clone(), v.frames)).collect();
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    let mut g = sh.lock();
                    let mut tracks = serde_json::Map::new();
                    let mine: Vec<String> = g.meters.keys().filter(|k| k.ends_with(&tag)).cloned().collect();
                    for k in mine { let m = g.meters.remove(&k).unwrap(); tracks.insert(k.trim_end_matches(&tag).to_string(), m.result()); }
                    for (k, v) in &g.video { tracks.insert(k.clone(), json!({"video_frames": v.frames - v0.get(k).copied().unwrap_or(0), "w": v.w, "h": v.h})); }
                    reply(&id, json!({"tracks": tracks, "e2ee": g.e2ee}));
                });
            }
            // patch W1: the playout gain of one sender's microphone, measured on the tap and on the receiver's playout stats
            "volume" => {
                let key = format!("{}/Microphone", c["identity"].as_str().unwrap_or(""));
                let t = shared.lock().audio_tracks.get(&key).cloned();
                match t {
                    Some(t) => {
                        t.rtc_track().set_volume(c["v"].as_f64().unwrap_or(1.));
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        let e0 = playout_energy(&t).await;
                        let mut m = tone::ToneMeter::new(c["hz"].as_f64().unwrap_or(500.));
                        let mut st = NativeAudioStream::new(t.rtc_track(), 48000, 1);
                        let end = tokio::time::Instant::now() + Duration::from_millis(2000);
                        while let Ok(Some(f)) = tokio::time::timeout_at(end, st.next()).await { m.push(&f.data, f.sample_rate, f.num_channels); }
                        let e1 = playout_energy(&t).await;
                        out(json!({"ok": true, "tap": m.result(), "playout_energy": e1 - e0}));
                    }
                    None => out(json!({"ok": false, "error": "no such track"})),
                }
            }
            "holds" => {
                let key = c["key"].as_str().and_then(keys::decode).unwrap_or_default();
                out(json!({"holds": book.lock().holds(c["identity"].as_str().unwrap_or(""), c["index"].as_i64().unwrap_or(0) as i32, &key)}));
            }
            "share" => {
                let (w, h) = (c["w"].as_u64().unwrap_or(2560) as u32, c["h"].as_u64().unwrap_or(1440) as u32);
                let src = NativeVideoSource::new(VideoResolution { width: w, height: h }, true);
                let track = LocalVideoTrack::create_video_track("screen", RtcVideoSource::Native(src.clone()));
                // call.js's "sharp text" profile: the full picture at 6 Mbit/s and 30 fps, plus one 960x540 layer at 350 kbit/s and 5 fps
                let o = TrackPublishOptions {
                    source: TrackSource::Screenshare, video_codec: VideoCodec::VP8, simulcast: true,
                    video_encoding: Some(VideoEncoding { max_bitrate: 6_000_000, max_framerate: 30.0 }),
                    simulcast_layers: Some(vec![VideoPreset::new(960, 540, 350_000, 5.0)]),
                    degradation_preference: Some(DegradationPreference::MaintainResolution), ..Default::default()
                };
                match room.local_participant().publish_track(LocalTrack::Video(track), o).await {
                    Ok(p) => {
                        if !index0 { book.lock().point_senders(room.e2ee_manager()); }
                        let feed = tokio::spawn(async move {
                            let mut t = tokio::time::interval(Duration::from_millis(33));
                            let mut n = 0;
                            loop { t.tick().await; src.capture_frame(&frame(w, h, n, (82, 90, 240))); n += 1; }   // about rgb(224, 32, 32)
                        });
                        out(json!({"ok": true, "sid": p.sid().to_string(), "encrypted": p.encryption_type() == EncryptionType::Gcm}));
                        share = Some((p, feed));
                    }
                    Err(e) => out(json!({"ok": false, "error": e.to_string()})),
                }
            }
            "unshare" => {
                if let Some((p, feed)) = share.take() { feed.abort(); let _ = room.local_participant().unpublish_track(&p.sid()).await; }
                out(json!({"ok": true}));
            }
            "status" => {
                let g = shared.lock();
                let senders: Vec<i32> = room.e2ee_manager().frame_cryptors().iter().filter(|((id, _), _)| id.as_str() == me).map(|(_, fc)| fc.key_index()).collect();
                let b = book.lock();
                out(json!({"state": format!("{:?}", room.connection_state()), "events": g.events, "e2ee": g.e2ee, "sender_index": senders, "index": b.index,
                    "epoch": b.epoch, "wipes": b.wipes_done, "republished_index": g.republished_index, "ice_seen": g.ice_seen,
                    "remote": room.remote_participants().keys().map(|k| k.to_string()).collect::<Vec<_>>()}));
            }
            "leave" => { let _ = room.close().await; out(json!({"ok": true})); break; }
            other => out(json!({"error": format!("unknown command {other}")})),
        }
    }
    std::process::exit(0);
}
