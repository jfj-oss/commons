//! Real-device probe (no network, silent): lists the platform's audio devices exactly as the app does, then, for each
//! `--mic <id|default>` argument, opens that capture device, measures the level a `NativeAudioStream` tap sees for a few
//! seconds and prints one JSON line. Nothing is ever played. Used by tools/gpui-realvoice.mjs.
use futures::StreamExt;
use livekit::prelude::*;
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let p = PlatformAudio::new().expect("PlatformAudio::new");
    let mics: Vec<_> = p.recording_devices().map(|d| json!({"id": d.id.as_str(), "name": d.name, "index": d.index})).collect();
    let outs: Vec<_> = p.playout_devices().map(|d| json!({"id": d.id.as_str(), "name": d.name, "index": d.index})).collect();
    println!("{}", json!({"devices": {"mic": mics, "out": outs}}));
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--mic" && i + 1 < args.len() {
            let id = args[i + 1].clone(); i += 2;
            let set = if id == "default" { Ok(()) } else { p.set_recording_device(&RecordingDeviceId::from_unchecked_guid(&id)).map_err(|e| e.to_string()) };
            let start = p.start_recording().map_err(|e| e.to_string());
            let track = LocalAudioTrack::create_device_audio_track_with_options("m", Default::default());
            let mut s = NativeAudioStream::new(track.rtc_track(), 48000, 1);
            let (mut n, mut sq, mut peak) = (0u64, 0f64, 0f64);
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_secs(3) {
                if let Ok(Some(f)) = tokio::time::timeout(Duration::from_millis(500), s.next()).await {
                    for x in f.data.iter() { let v = *x as f64 / 32768.; sq += v * v; peak = peak.max(v.abs()); n += 1; }
                }
            }
            let rms = if n > 0 { (sq / n as f64).sqrt() } else { -1. };
            println!("{}", json!({"mic": id, "set": format!("{set:?}"), "start": format!("{start:?}"), "samples": n, "rms": rms, "peak": peak}));
            let _ = p.stop_recording();
        } else { i += 1; }
    }
}
