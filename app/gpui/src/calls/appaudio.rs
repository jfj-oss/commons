//! One app's sound, for the call (CALLS-PLAN.md section 7, the "PC audio, one app" row, done without the core's virtual
//! source): the playback streams PipeWire (through its PulseAudio interface) knows about, and a capture of exactly one
//! of them.
//!
//! Listing: `pactl -f json list sink-inputs` and `... list sinks`, one fixed argument list, no shell. Each stream is an
//! [`AppStream`]: its id, the application's name, its process id, its media name (the song or the page), the sink it
//! plays to. The call's own sound (this process, and the core's `pw-loopback` helpers) is left out; a browser is listed
//! with `browser: true` (the native app, unlike the web page, is not what plays the call, so its sound can be shared).
//!
//! Capture: `parec --monitor-stream=<id> --device=<sink>.monitor --format=s16le --rate=48000 --channels=2 --raw`, a child
//! process that tees that one stream's sound off, before the sink. The app keeps playing where it was, nothing is moved,
//! and nothing else on the sink is heard. 10 ms chunks (480 stereo samples) go to the publisher. The capture ends by
//! itself when the app closes the stream or `parec` dies, and the publisher is told.
use serde::Serialize;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use tokio::sync::mpsc::{channel, Receiver};

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct AppStream {
    /// The sink-input's index: what to pass to [`Command::StartShareAudio`](super::engine::Command).
    pub id: u32,
    /// `application.name` (or the program's name when there is none).
    pub app: String,
    pub pid: Option<u32>,
    /// `media.name`: the song, the video, the page.
    pub media: String,
    pub binary: String,
    /// The sink's name, empty while the stream is not linked to any (its sound goes nowhere).
    pub sink: String,
    pub corked: bool,
    pub browser: bool,
}

fn clip(s: &str) -> String { s.trim().chars().filter(|c| !c.is_control()).take(120).collect() }

/// `pactl -f json list sink-inputs`, with `sinks` (index, name) to name where each plays.
pub fn parse_streams(inputs: &str, sinks: &[(u32, String)], own_pid: u32) -> Result<Vec<AppStream>, String> {
    let v: serde_json::Value = serde_json::from_str(inputs).map_err(|e| format!("pactl's answer could not be read: {e}"))?;
    let list = v.as_array().ok_or("pactl's answer was not a list")?;
    let mut out = vec![];
    for it in list {
        let Some(id) = it["index"].as_u64().and_then(|n| u32::try_from(n).ok()) else { continue };
        let p = &it["properties"];
        let get = |k: &str| clip(p[k].as_str().unwrap_or(""));
        let binary = get("application.process.binary");
        let app = { let a = get("application.name"); if a.is_empty() { binary.clone() } else { a } };
        let pid = p["application.process.id"].as_str().and_then(|x| x.parse::<u32>().ok()).or_else(|| p["application.process.id"].as_u64().and_then(|n| u32::try_from(n).ok()));
        let sink = it["sink"].as_u64().and_then(|n| u32::try_from(n).ok()).and_then(|i| sinks.iter().find(|(x, _)| *x == i)).map(|(_, n)| n.clone()).unwrap_or_default();
        let s = commons_core::pcaudio::Stream { id, app: app.clone(), title: get("media.name"), binary: binary.clone(), sink: None, corked: false };
        if pid == Some(own_pid) || commons_core::pcaudio::is_ours(&s) { continue; }
        out.push(AppStream { id, app, pid, media: get("media.name"), browser: commons_core::pcaudio::is_browser(&s), binary, sink, corked: it["corked"].as_bool().unwrap_or(false) });
    }
    Ok(out)
}

fn pactl(args: &[&str]) -> Result<String, String> {
    let o = Command::new("pactl").args(args).stdin(Stdio::null()).output().map_err(|e| format!("pactl: {e}"))?;
    if !o.status.success() { return Err(format!("pactl {}: {}", args.join(" "), String::from_utf8_lossy(&o.stderr).trim())); }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

/// The apps playing sound now. Blocking: run it off the actor.
pub fn list() -> Result<Vec<AppStream>, String> {
    let sinks_json = pactl(&["-f", "json", "list", "sinks"])?;
    let sinks: Vec<(u32, String)> = serde_json::from_str::<serde_json::Value>(&sinks_json).ok().and_then(|v| v.as_array().map(|a| a.iter().filter_map(|it| Some((u32::try_from(it["index"].as_u64()?).ok()?, it["name"].as_str()?.to_string()))).collect())).unwrap_or_default();
    parse_streams(&pactl(&["-f", "json", "list", "sink-inputs"])?, &sinks, std::process::id())
}

/// One stream by id, as it is now.
pub fn find(id: u32) -> Result<AppStream, String> {
    list()?.into_iter().find(|s| s.id == id).ok_or_else(|| "That app is not playing any sound any more.".to_string())
}

/// A sink name we are willing to hand to a tool as one argument.
pub fn valid_name(s: &str) -> bool { commons_core::pcaudio::valid_name(s) }

/// The running capture: 10 ms stereo chunks on `rx`, which closes when the capture ends.
pub struct Tap {
    pub rx: Receiver<Vec<i16>>,
    child: Arc<parking_lot::Mutex<Child>>,
}
impl Tap {
    pub fn stop(&self) { let mut c = self.child.lock(); let _ = c.kill(); let _ = c.wait(); }
}
impl Drop for Tap { fn drop(&mut self) { self.stop(); } }

/// Samples in one chunk: 10 ms at 48 kHz, two channels.
pub const CHUNK: usize = 480 * 2;

pub fn tap(s: &AppStream) -> Result<Tap, String> {
    if s.sink.is_empty() { return Err("That app's sound is not playing to any output, so there is nothing to share.".into()); }
    if !valid_name(&s.sink) { return Err("That app's output has a name that cannot be used.".into()); }
    let mut child = Command::new("parec")
        .args([&format!("--monitor-stream={}", s.id), &format!("--device={}.monitor", s.sink), "--format=s16le", "--rate=48000", "--channels=2", "--latency-msec=20", "--raw", "--client-name=mega-share-audio"])
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| format!("parec (from pulseaudio-utils) could not be started: {e}"))?;
    let mut out = child.stdout.take().ok_or("parec has no output")?;
    let child = Arc::new(parking_lot::Mutex::new(child));
    let (tx, rx) = channel::<Vec<i16>>(64);
    std::thread::Builder::new().name("mega-share-audio".into()).spawn(move || {
        let mut raw = [0u8; CHUNK * 2];
        while out.read_exact(&mut raw).is_ok() {
            let v: Vec<i16> = raw.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
            if tx.blocking_send(v).is_err() { break; }
        }
    }).map_err(|e| format!("audio thread: {e}"))?;
    Ok(Tap { rx, child })
}

#[cfg(test)]
mod tests {
    use super::*;
    const INPUTS: &str = r#"[
      {"index": 38, "sink": 36, "corked": false, "properties": {"media.name": "Test song", "application.name": "Spotify", "application.process.id": "2563401", "application.process.binary": "spotify"}},
      {"index": 41, "sink": 36, "corked": true, "properties": {"media.name": "A tab", "application.name": "Firefox", "application.process.id": "77", "application.process.binary": "firefox"}},
      {"index": 50, "sink": 36, "properties": {"media.name": "the call", "application.name": "mega-gpui", "application.process.id": "999", "application.process.binary": "mega-gpui"}},
      {"index": 51, "sink": 36, "properties": {"media.name": "output", "application.name": "megatron_pcaudio_listen", "application.process.binary": "pw-loopback"}},
      {"index": 52, "sink": 4294967295, "properties": {"media.name": "x", "application.process.binary": "mpv"}},
      {"sink": 36, "properties": {}}
    ]"#;
    #[test]
    fn streams_are_listed_without_our_own_sound() {
        let v = parse_streams(INPUTS, &[(36, "alsa_output.main".into())], 999).unwrap();
        assert_eq!(v.iter().map(|s| s.id).collect::<Vec<_>>(), [38, 41, 52]);
        assert_eq!((v[0].app.as_str(), v[0].pid, v[0].media.as_str(), v[0].sink.as_str()), ("Spotify", Some(2563401), "Test song", "alsa_output.main"));
        assert!(!v[0].browser && v[1].browser && v[1].corked);
        assert_eq!((v[2].app.as_str(), v[2].pid, v[2].sink.as_str()), ("mpv", None, ""));
    }
    #[test]
    fn a_bad_answer_is_an_error() { assert!(parse_streams("nope", &[], 1).is_err()); assert!(parse_streams("{}", &[], 1).is_err()); }
}
