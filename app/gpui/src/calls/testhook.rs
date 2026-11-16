//! Tests only: a line-JSON control port on 127.0.0.1, opened only when `MEGA_GPUI_CALL_TEST=<port>` is set. One JSON
//! object per line in, one per line out (with the command's `id`). Used by tools/gpui-call-e2e.mjs (and the voice
//! test) against `mega-gpui --call-headless`.
//!   {"cmd":"join","gid","name"} {"cmd":"leave"} {"cmd":"mute","on"} {"cmd":"deafen","on"} {"cmd":"volume","account","v"}
//!   {"cmd":"snapshot"} {"cmd":"hear","identity","hz","ms"} {"cmd":"game","muted":true|false|null} {"cmd":"trust_all","on"}
//!   {"cmd":"devices"} {"cmd":"mic","id"} {"cmd":"out","id"} {"cmd":"processing","echo","noise","gain"}
//!   {"cmd":"ptt_mode","on"} {"cmd":"ptt","down"} {"cmd":"plout","identity","ms"} {"cmd":"mic_options"}
//!   {"cmd":"core","method","args"} {"cmd":"call_active"} {"cmd":"quit"}
use super::audio::Processing;
use super::engine::{CallEngine, Command};
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
        "mic" => { e.send(Command::SetMicDevice { id: s("id") }); json!({"ok": true}) }
        "out" => { e.send(Command::SetOutDevice { id: s("id") }); json!({"ok": true}) }
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
        "ptt" => { e.send(Command::PushToTalk(c["down"].as_bool().unwrap_or(on))); json!({"ok": true}) }
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
