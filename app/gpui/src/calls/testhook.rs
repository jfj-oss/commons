//! Tests only: a line-JSON control port on 127.0.0.1, opened only when `MEGA_GPUI_CALL_TEST=<port>` is set. One JSON
//! object per line in, one per line out (with the command's `id`). Used by tools/gpui-call-e2e.mjs (and the voice
//! test) against `mega-gpui --call-headless`.
//!   {"cmd":"join","gid","name"} {"cmd":"leave"} {"cmd":"mute","on"} {"cmd":"deafen","on"} {"cmd":"volume","account","v"}
//!   {"cmd":"snapshot"} {"cmd":"hear","identity","hz","ms"} {"cmd":"game","muted":true|false|null} {"cmd":"trust_all","on"}
//!   {"cmd":"devices"} {"cmd":"mic","id"} {"cmd":"out","id"} {"cmd":"processing","echo","noise","gain"}
//!   {"cmd":"ptt_mode","on"} {"cmd":"ptt","down"} {"cmd":"plout","identity","ms"} {"cmd":"mic_options"}
//!   {"cmd":"share","profile":"text|motion|motion60","source":"test|portal","w","h","speed","audio":<stream id>}  {"cmd":"share_stop"}
//!   {"cmd":"share_switch","source","w","h","speed"} {"cmd":"share_profile","profile"} {"cmd":"share_end"} {"cmd":"share_stats"}
//!   {"cmd":"share_apps"} {"cmd":"share_audio","stream"} {"cmd":"share_audio_stop"}
//!   {"cmd":"ptt_mode","on"} {"cmd":"ptt","down"} {"cmd":"shortcut","id","down"} {"cmd":"rebind"} {"cmd":"plout","identity","ms"} {"cmd":"mic_options"}
//!   {"cmd":"cameras"} {"cmd":"camera","source":"test|device","id","profile":"720p|1080p"} {"cmd":"camera_stop"} {"cmd":"camera_switch","source","id"}
//!   {"cmd":"camera_profile","profile"} {"cmd":"camera_end"}
//!   {"cmd":"frames"} (per tile: count, w, h, and frames per second over the last `ms`) {"cmd":"frames_hide","sid","on"}
//!   {"cmd":"core","method","args"} {"cmd":"call_active"} {"cmd":"quit"}
use super::audio::Processing;
use super::capture::{ShareSource, TestPattern};
use super::camera::{CamProfile, CameraSource};
use super::engine::{CallEngine, Command};
use super::share::Profile;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

pub fn port() -> Option<u16> { std::env::var("MEGA_GPUI_CALL_TEST").ok().and_then(|p| p.parse().ok()) }

/// Serves until a `quit` arrives; returns then.
pub async fn serve(engine: Arc<CallEngine>, port: u16) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let (quit_tx, mut quit_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    loop {
        let (sock, _) = tokio::select! { a = listener.accept() => a?, _ = quit_rx.recv() => return Ok(()) };
        let (e, q) = (engine.clone(), quit_tx.clone());
        tokio::spawn(async move {
            let (r, w) = sock.into_split();
            let w = Arc::new(tokio::sync::Mutex::new(w));
            let mut lines = tokio::io::BufReader::new(r).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let (e, w, q) = (e.clone(), w.clone(), q.clone());
                // each command on its own, so a long `hear` does not hold up the rest
                tokio::spawn(async move {
                    let c: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                    let mut out = handle(&e, &c, &q).await;
                    out["id"] = c["id"].clone();
                    let mut w = w.lock().await;
                    let _ = w.write_all(format!("{out}\n").as_bytes()).await;
                });
            }
        });
    }
}

async fn handle(e: &Arc<CallEngine>, c: &Value, quit: &tokio::sync::mpsc::UnboundedSender<()>) -> Value {
    let s = |k: &str| c[k].as_str().unwrap_or("").to_string();
    let on = c["on"].as_bool().unwrap_or(false);
    match c["cmd"].as_str().unwrap_or("") {
        "join" => { e.send(Command::Join { gid: s("gid"), name: s("name") }); json!({"ok": true}) }
        "leave" => { e.send(Command::Leave); json!({"ok": true}) }
        "mute" => { e.send(Command::SetMuted(on)); json!({"ok": true}) }
        "deafen" => { e.send(Command::SetDeafened(on)); json!({"ok": true}) }
        "volume" => { e.send(Command::SetVolume { account: s("account"), volume: c["v"].as_f64().unwrap_or(1.) }); json!({"ok": true}) }
        "game" => { e.send(Command::TestGame(c["muted"].as_bool())); json!({"ok": true}) }
        "trust_all" => { e.send(Command::TestTrustAll(on)); json!({"ok": true}) }
        "devices" => devices(e).await,
        "mic" => { e.send(Command::SetMicDevice { id: s("device") }); json!({"ok": true}) }
        "out" => { e.send(Command::SetOutDevice { id: s("device") }); json!({"ok": true}) }
        "processing" => {
            let cur = e.snapshot();
            let p = Processing {
                echo_cancellation: c["echo"].as_bool().unwrap_or(cur.echo_cancellation),
                noise_suppression: c["noise"].as_bool().unwrap_or(cur.noise_suppression),
                auto_gain_control: c["gain"].as_bool().unwrap_or(cur.auto_gain_control),
            };
            e.send(Command::SetProcessing(p)); json!({"ok": true})
        }
        "ptt_mode" => { e.send(Command::SetPushToTalk(on)); json!({"ok": true}) }
        "shortcut" => match super::shortcuts::Shortcut::from_id(&s("id")) {
            Some(sc) => { e.send(Command::Shortcut { shortcut: sc, down: c["down"].as_bool().unwrap_or(on) }); json!({"ok": true}) }
            None => json!({"error": "unknown shortcut id"}),
        },
        "rebind" => { e.send(Command::RebindShortcuts); json!({"ok": true}) }
        "ptt" => { e.send(Command::PushToTalk(c["down"].as_bool().unwrap_or(on))); json!({"ok": true}) }
        "share" => {
            let Some(profile) = Profile::parse(c["profile"].as_str().unwrap_or("text")) else { return json!({"error": "unknown profile"}) };
            e.send(Command::StartShare { profile, source: source(c), audio: c["audio"].as_u64().map(|n| n as u32) }); json!({"ok": true})
        }
        "share_stop" => { e.send(Command::StopShare); json!({"ok": true}) }
        "share_switch" => { e.send(Command::SwitchShareSource(source(c))); json!({"ok": true}) }
        "share_profile" => {
            let Some(profile) = Profile::parse(c["profile"].as_str().unwrap_or("")) else { return json!({"error": "unknown profile"}) };
            e.send(Command::SetShareProfile(profile)); json!({"ok": true})
        }
        "share_stats" => e.share_stats().await,
        "share_end" => { e.send(Command::TestShareEnd); json!({"ok": true}) }
        "share_apps" => {
            e.send(Command::ListShareApps);
            tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
            json!({"apps": e.snapshot().share_apps})
        }
        "share_audio" => { e.send(Command::StartShareAudio { stream: c["stream"].as_u64().unwrap_or(0) as u32 }); json!({"ok": true}) }
        "share_audio_stop" => { e.send(Command::StopShareAudio); json!({"ok": true}) }
        "cameras" => {
            e.send(Command::ListCameras);
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            json!({"cameras": e.snapshot().cameras})
        }
        "camera" => { e.send(Command::StartCamera { source: cam_source(c), profile: CamProfile::parse(c["profile"].as_str().unwrap_or("")) }); json!({"ok": true}) }
        "camera_stop" => { e.send(Command::StopCamera); json!({"ok": true}) }
        "camera_switch" => { e.send(Command::SwitchCamera(cam_source(c))); json!({"ok": true}) }
        "camera_profile" => {
            let Some(p) = CamProfile::parse(c["profile"].as_str().unwrap_or("")) else { return json!({"error": "unknown profile"}) };
            e.send(Command::SetCameraProfile(p)); json!({"ok": true})
        }
        "camera_end" => { e.send(Command::TestCameraEnd); json!({"ok": true}) }
        "frames" => match &e.frames {
            None => json!({"error": "no frames sink"}),
            Some(f) => {
                // counters now and `ms` later: the difference is the rate the window would have seen
                let ms = c["ms"].as_u64().unwrap_or(0);
                let a = f.stats();
                if ms > 0 { tokio::time::sleep(std::time::Duration::from_millis(ms)).await; }
                let b = f.stats();
                let tiles: serde_json::Map<String, Value> = b.iter().map(|(k, v)| {
                    let before = a.get(k).map(|x| x.count).unwrap_or(0);
                    (k.clone(), json!({"count": v.count, "w": v.w, "h": v.h, "fps": if ms > 0 { (v.count - before.min(v.count)) as f64 * 1000. / ms as f64 } else { 0. }, "hidden": f.is_hidden(k)}))
                }).collect();
                json!({"tiles": tiles})
            }
        },
        "frames_hide" => { if let Some(f) = &e.frames { f.set_hidden(&s("sid"), on); } json!({"ok": true}) }
        "snapshot" => serde_json::to_value(e.snapshot()).unwrap_or(Value::Null),
        "hear" => e.taps.hear(&s("identity"), c["hz"].as_f64().unwrap_or(500.), c["ms"].as_u64().unwrap_or(1500)).await,
        "plout" => e.plout(&s("identity"), c["ms"].as_u64().unwrap_or(1500)).await,
        "mic_options" => e.mic_options().await,
        "core" => {
            let (core, m, a) = (e.core.clone(), s("method"), c["args"].clone());
            match tokio::task::spawn_blocking(move || core.call(&m, a)).await { Ok(Ok(v)) => json!({"ok": v}), Ok(Err(x)) => json!({"err": x}), Err(x) => json!({"err": x.to_string()}) }
        }
        "call_active" => json!({"active": e.core.call_active()}),
        "quit" => { e.send(Command::Leave); let _ = quit.send(()); json!({"ok": true}) }
        other => json!({"error": format!("unknown command {other}")}),
    }
}

/// `source: "portal"` opens the system's picker (never in the automated tests); the default is the synthetic test source.
fn source(c: &Value) -> ShareSource {
    if c["source"].as_str() == Some("portal") { return ShareSource::Portal; }
    let d = TestPattern::default();
    ShareSource::Test(TestPattern { width: c["w"].as_u64().map(|n| n as u32).unwrap_or(d.width), height: c["h"].as_u64().map(|n| n as u32).unwrap_or(d.height), speed: c["speed"].as_u64().map(|n| n as u32).unwrap_or(d.speed) })
}

fn cam_source(c: &Value) -> CameraSource {
    if c["source"].as_str() == Some("test") { CameraSource::Test } else { CameraSource::Device(c["id"].as_str().map(String::from)) }
}

async fn devices(e: &Arc<CallEngine>) -> Value {
    e.send(Command::ListDevices);
    let mut v = e.changes();
    let mut d = e.snapshot().devices.clone();
    for _ in 0..40 {
        if !d.mic.is_empty() || !d.out.is_empty() { break; }
        let _ = tokio::time::timeout(std::time::Duration::from_millis(50), v.changed()).await;
        d = e.snapshot().devices.clone();
    }
    serde_json::to_value(d).unwrap_or(Value::Null)
}
