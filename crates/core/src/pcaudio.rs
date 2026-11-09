//! "Play PC audio into the call", route 2: send the sound of ONE app on this Linux computer (Spotify desktop or anything
//! else) to the call, through a virtual PipeWire sink and source that exist only while it is being shared.
//!
//! How it works, all through `pactl` and `pw-loopback` with fixed argument lists (no shell, ids checked as numbers):
//!  * `pw-loopback` makes a virtual sink (`megatron_pcaudio_sink`) and a virtual source ("Megatron PC audio",
//!    `megatron_pcaudio_src`). Neither is ever the default device (low session priority, `node.dont-fallback`), and the
//!    defaults are read before and after and put back if anything changed them.
//!  * The one chosen stream is moved onto the virtual sink with `pactl move-sink-input`. A second `pw-loopback` plays that
//!    sink to the device the stream was on before, so the person keeps hearing it once, at normal volume.
//!  * The page picks the virtual source by its label and publishes it like any other music.
//!  * On stop, on a missed heartbeat (page closed or crashed), on bridge shutdown, or when a loopback dies: the stream is
//!    moved back and both loopbacks are ended. Stale loopbacks of ours from a crashed bridge are ended on the next start,
//!    matched by their exact node names only.
//!
//! The call's own audio is never offered: browsers are left out of the list (their streams cannot be told apart per tab,
//! and one of them is playing the call), and the default output's monitor is never captured.
use serde_json::{json, Value};
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const SINK: &str = "megatron_pcaudio_sink";
pub const SOURCE: &str = "megatron_pcaudio_src";
/// What the browser lists the virtual input as; the page finds it by this exact label.
pub const LABEL: &str = "Megatron PC audio";
pub const LOOP_NAME: &str = "megatron_pcaudio_loop";
pub const LISTEN_NAME: &str = "megatron_pcaudio_listen";
const LISTEN_IN: &str = "megatron_pcaudio_listen_in";
const LISTEN_OUT: &str = "megatron_pcaudio_listen_out";
/// The page asks for the status every second; without that for this long, everything is undone.
const LEASE: Duration = Duration::from_secs(20);
const TOOL_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Stream {
    pub id: u32,
    pub app: String,
    pub title: String,
    pub binary: String,
    pub sink: Option<u32>,
    pub corked: bool,
}

fn clip(s: &str) -> String { s.trim().chars().filter(|c| !c.is_control()).take(120).collect() }

/// `pactl -f json list sink-inputs`. Entries without a usable numeric index are skipped.
pub fn parse_sink_inputs(json: &str) -> Result<Vec<Stream>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("pactl's answer could not be read: {e}"))?;
    let list = v.as_array().ok_or("pactl's answer was not a list")?;
    let mut out = vec![];
    for it in list {
        let Some(id) = it["index"].as_u64().and_then(|n| u32::try_from(n).ok()) else { continue };
        let p = &it["properties"];
        let get = |k: &str| clip(p[k].as_str().unwrap_or(""));
        let app = { let a = get("application.name"); if a.is_empty() { get("application.process.binary") } else { a } };
        out.push(Stream {
            id, app, title: get("media.name"), binary: get("application.process.binary"),
            sink: it["sink"].as_u64().and_then(|n| u32::try_from(n).ok()),
            corked: it["corked"].as_bool().unwrap_or(false),
        });
    }
    Ok(out)
}

/// `pactl -f json list sinks`, as (index, name).
pub fn parse_sinks(json: &str) -> Result<Vec<(u32, String)>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("pactl's answer could not be read: {e}"))?;
    let list = v.as_array().ok_or("pactl's answer was not a list")?;
    Ok(list.iter().filter_map(|it| Some((u32::try_from(it["index"].as_u64()?).ok()?, it["name"].as_str()?.to_string()))).collect())
}

const BROWSERS: &[&str] = &["chrome", "chromium", "brave", "firefox", "vivaldi", "opera", "msedge", "microsoft-edge", "librewolf", "waterfox", "floorp", "thorium", "epiphany", "falkon", "tor-browser", "zen", "mullvad", "google-chrome", "ungoogled"];
const BROWSER_APPS: &[&str] = &["chrom", "brave", "firefox", "vivaldi", "opera", "microsoft edge", "librewolf", "waterfox", "floorp", "thorium", "epiphany", "falkon", "tor browser", "zen browser", "mullvad"];

/// A web browser (or anything of ours): one of them is running this call, and per-tab streams cannot be told apart.
pub fn is_browser(s: &Stream) -> bool {
    let bin = s.binary.rsplit('/').next().unwrap_or("").to_ascii_lowercase();
    let app = s.app.to_ascii_lowercase();
    BROWSERS.iter().any(|b| bin == *b || bin.starts_with(&format!("{b}-")) || bin.starts_with(&format!("{b}_"))) || BROWSER_APPS.iter().any(|b| app.contains(b))
}
pub fn is_ours(s: &Stream) -> bool {
    let bin = s.binary.rsplit('/').next().unwrap_or("");
    bin == "pw-loopback" || [LOOP_NAME, LISTEN_NAME, SINK, SOURCE, LISTEN_IN, LISTEN_OUT].iter().any(|n| s.app.contains(n) || s.title.contains(n))
}
/// Why a stream is not offered, if it is not.
pub fn refusal(s: &Stream) -> Option<&'static str> {
    if is_ours(s) { Some("That is Megatron's own sound.") }
    else if is_browser(s) { Some("Browser sound is not offered here, because the call itself plays from a browser. Use \"A browser tab\" instead.") }
    else { None }
}

/// A device or program name we are willing to hand to a tool as one argument: no leading dash, plain characters only.
pub fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 200 && !s.starts_with('-') && s.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b':' | b'@' | b'+' | b'-'))
}
/// A stream id from the page: a JSON integer that fits a u32. Strings, floats and negatives are refused.
pub fn parse_id(v: &Value) -> Result<u32, String> {
    v.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| "That is not a valid stream number.".to_string())
}

pub fn loop_args() -> Vec<String> {
    let props_in = format!("media.class=Audio/Sink node.name={SINK} node.description=\"Megatron app capture\" priority.session=1 priority.driver=1 node.dont-fallback=true node.dont-reconnect=true node.passive=true");
    let props_out = format!("media.class=Audio/Source node.name={SOURCE} node.description=\"{LABEL}\" priority.session=1 priority.driver=1 node.dont-fallback=true node.dont-reconnect=true");
    ["-n", LOOP_NAME, "-c", "2", "-m", "[ FL, FR ]", "-i", &props_in, "-o", &props_out].iter().map(|s| s.to_string()).collect()
}
/// Plays the virtual sink to the device the stream used to be on. `prev` must pass `valid_name`.
pub fn listen_args(prev: &str) -> Vec<String> {
    let props_in = format!("node.name={LISTEN_IN} stream.capture.sink=true node.dont-fallback=true node.dont-reconnect=true");
    let props_out = format!("node.name={LISTEN_OUT} node.dont-fallback=true node.dont-reconnect=true");
    ["-n", LISTEN_NAME, "-c", "2", "-m", "[ FL, FR ]", "-C", SINK, "-P", prev, "-i", &props_in, "-o", &props_out].iter().map(|s| s.to_string()).collect()
}
pub fn move_args(id: u32, target: &str) -> Vec<String> { vec!["move-sink-input".into(), id.to_string(), target.into()] }

/// Is this command line (from /proc/<pid>/cmdline) one of OUR pw-loopbacks? Matched by the exact `-n` node names only.
pub fn cmdline_is_ours(args: &[String]) -> bool {
    let is_loop = args.iter().take(2).any(|a| a.rsplit('/').next() == Some("pw-loopback"));
    if !is_loop { return false; }
    let ours = |n: &str| n == LOOP_NAME || n == LISTEN_NAME;
    args.windows(2).any(|w| (w[0] == "-n" || w[0] == "--name") && ours(&w[1])) || args.iter().any(|a| a.strip_prefix("--name=").is_some_and(ours))
}

// ------------------------------------------------------------------ running the tools
fn run_capture(prog: &str, args: &[String]) -> Result<String, String> {
    let mut c = Command::new(prog).args(args).env("LC_ALL", "C").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
        .map_err(|e| format!("{prog} could not be started: {e}"))?;
    let mut so = c.stdout.take().ok_or("no output")?;
    let mut se = c.stderr.take().ok_or("no output")?;
    let h = std::thread::spawn(move || { let mut s = String::new(); let _ = so.by_ref().take(8 << 20).read_to_string(&mut s); s });
    let h2 = std::thread::spawn(move || { let mut s = String::new(); let _ = se.by_ref().take(1 << 16).read_to_string(&mut s); s });
    let t0 = Instant::now();
    let status = loop {
        match c.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if t0.elapsed() > TOOL_TIMEOUT => { let _ = c.kill(); let _ = c.wait(); return Err(format!("{prog} did not answer in time")); }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(e) => return Err(format!("{prog}: {e}")),
        }
    };
    let (out, err) = (h.join().unwrap_or_default(), h2.join().unwrap_or_default());
    if status.success() { Ok(out) } else { Err(format!("{prog} failed: {}", err.trim().lines().next().unwrap_or("no reason given"))) }
}
fn pactl(args: &[&str]) -> Result<String, String> { run_capture("pactl", &args.iter().map(|s| s.to_string()).collect::<Vec<_>>()) }
fn list_streams() -> Result<Vec<Stream>, String> { parse_sink_inputs(&pactl(&["-f", "json", "list", "sink-inputs"])?) }
fn list_sinks() -> Result<Vec<(u32, String)>, String> { parse_sinks(&pactl(&["-f", "json", "list", "sinks"])?) }
fn defaults() -> (String, String) {
    let g = |k: &str| pactl(&[k]).map(|s| s.trim().to_string()).unwrap_or_default();
    (g("get-default-sink"), g("get-default-source"))
}
fn on_path(prog: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(prog).is_file()))
}

/// Children are started from one long-lived thread: Linux ends a child with the thread that started it (parent-death
/// signal), and the request threads of the bridge are short-lived.
type SpawnReq = (Command, mpsc::Sender<std::io::Result<Child>>);
fn spawn_child(mut cmd: Command) -> Result<Child, String> {
    static TX: OnceLock<Mutex<mpsc::Sender<SpawnReq>>> = OnceLock::new();
    let tx = TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<SpawnReq>();
        std::thread::spawn(move || for (mut c, back) in rx { let _ = back.send(c.spawn()); });
        Mutex::new(tx)
    });
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // If the bridge dies in any way, the kernel ends the loopback and PipeWire drops the virtual devices.
        unsafe { cmd.pre_exec(|| { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM); Ok(()) }); }
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let (back, got) = mpsc::channel();
    tx.lock().unwrap().send((cmd, back)).map_err(|_| "the helper thread is gone".to_string())?;
    got.recv_timeout(TOOL_TIMEOUT).map_err(|_| "pw-loopback did not start in time".to_string())?.map_err(|e| format!("pw-loopback could not be started: {e}"))
}

// ------------------------------------------------------------------ state
struct Active {
    stream: Stream,
    /// Where the stream was before: the sink's index and the name the listening loop plays to.
    orig_sink: u32,
    procs: Vec<Child>,
    seen: Instant,
    gone_since: Option<Instant>,
}
static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);
static ENDED: Mutex<Option<String>> = Mutex::new(None);

fn linux_only() -> Result<(), String> {
    if cfg!(target_os = "linux") { Ok(()) } else { Err("Sharing an app's sound is only available on Linux. Use \"A browser tab\" instead.".into()) }
}
fn tools() -> Result<(), String> {
    if !on_path("pactl") || !on_path("pw-loopback") { return Err("This needs PipeWire with pactl and pw-loopback installed (they were not found).".into()); }
    Ok(())
}

fn kill_all(procs: &mut Vec<Child>) {
    for c in procs.iter_mut() { let _ = c.kill(); }
    for c in procs.iter_mut() { let _ = c.wait(); }
    procs.clear();
}

/// Put everything back. Safe to call with a half-built state.
fn undo(mut a: Active) {
    if let (Ok(sinks), Ok(streams)) = (list_sinks(), list_streams()) {
        let ours = sinks.iter().find(|(_, n)| n == SINK).map(|(i, _)| *i);
        // Only a stream that is still on our sink is moved; if the person moved it elsewhere themselves, it stays there.
        if let Some(s) = streams.iter().find(|s| s.id == a.stream.id && s.app == a.stream.app && s.binary == a.stream.binary) {
            // (If our sink is already gone, PipeWire has moved the stream somewhere on its own; put it back where it was.)
            if s.sink.is_some() && (s.sink == ours || (ours.is_none() && s.sink != Some(a.orig_sink))) {
                let back = if sinks.iter().any(|(i, _)| *i == a.orig_sink) { a.orig_sink.to_string() } else { "@DEFAULT_SINK@".to_string() };
                if run_capture("pactl", &move_args(a.stream.id, &back)).is_err() { let _ = run_capture("pactl", &move_args(a.stream.id, "@DEFAULT_SINK@")); }
            }
        }
    }
    kill_all(&mut a.procs);
}

/// End loopbacks of ours that a crashed bridge left behind: exact node names, our own user, never our current children.
pub fn cleanup_stale() -> usize {
    let mine: Vec<u32> = ACTIVE.lock().unwrap().as_ref().map(|a| a.procs.iter().map(|c| c.id()).collect()).unwrap_or_default();
    cleanup_stale_except(&mine)
}
fn cleanup_stale_except(mine: &[u32]) -> usize {
    #[cfg(not(target_os = "linux"))]
    { let _ = mine; 0 }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        let me = std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(u32::MAX);
        let mut n = 0;
        let Ok(rd) = std::fs::read_dir("/proc") else { return 0 };
        for e in rd.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
            if pid <= 1 || mine.contains(&pid) { continue; }
            if std::fs::metadata(e.path()).map(|m| m.uid()) .ok() != Some(me) { continue; }
            let Ok(raw) = std::fs::read(e.path().join("cmdline")) else { continue };
            let args: Vec<String> = raw.split(|b| *b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect();
            if cmdline_is_ours(&args) { unsafe { libc::kill(pid as i32, libc::SIGTERM); } n += 1; }
        }
        n
    }
}

fn watchdog() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| { std::thread::spawn(|| loop {
        std::thread::sleep(Duration::from_secs(1));
        let mut g = ACTIVE.lock().unwrap();
        let Some(a) = g.as_mut() else { continue };
        let dead = a.procs.iter_mut().any(|c| matches!(c.try_wait(), Ok(Some(_))));
        let why = if dead { Some("The sound route stopped unexpectedly, so sharing was stopped.") } else if a.seen.elapsed() > LEASE { Some("The call page went away, so sharing was stopped.") } else { None };
        if let Some(w) = why { let a = g.take().unwrap(); *ENDED.lock().unwrap() = Some(w.into()); drop(g); undo(a); }
    }); });
}

// ------------------------------------------------------------------ the operations
pub fn caps() -> Value {
    if cfg!(target_os = "linux") { cleanup_stale_once(); }
    json!({"supported": cfg!(target_os = "linux") && on_path("pactl") && on_path("pw-loopback"), "linux": cfg!(target_os = "linux"), "label": LABEL})
}
fn cleanup_stale_once() { static ONCE: OnceLock<()> = OnceLock::new(); ONCE.get_or_init(|| { cleanup_stale(); }); }

pub fn streams() -> Result<Value, String> {
    linux_only()?; tools()?;
    let all = list_streams()?;
    let hidden = all.iter().filter(|s| refusal(s).is_some()).count();
    let offered: Vec<&Stream> = all.iter().filter(|s| refusal(s).is_none()).collect();
    Ok(json!({"streams": offered, "browser_hidden": all.iter().filter(|s| is_browser(s) && !is_ours(s)).count(), "hidden": hidden}))
}

pub fn start(a: &Value) -> Result<Value, String> {
    linux_only()?; tools()?;
    let id = parse_id(&a["id"])?;
    let mut g = ACTIVE.lock().unwrap();
    if g.is_some() { return Err("An app's sound is already being shared. Stop it first.".into()); }
    cleanup_stale_except(&[]);   // nothing of ours is running (checked above), and the lock is held
    *ENDED.lock().unwrap() = None;
    let all = list_streams()?;
    let s = all.iter().find(|s| s.id == id).cloned().ok_or("That app is not playing any more. Refresh the list.")?;
    if let Some(why) = refusal(&s) { return Err(why.into()); }
    let sinks = list_sinks()?;
    if sinks.iter().any(|(_, n)| n == SINK) { return Err("A sound route of ours is still in place. Try again in a moment.".into()); }
    let orig = s.sink.ok_or("Could not tell where that app is playing.")?;
    let prev = sinks.iter().find(|(i, _)| *i == orig).map(|(_, n)| n.clone()).filter(|n| valid_name(n))
        .ok_or("Could not find the speakers that app is playing to, so it was left alone.")?;
    let before = defaults();
    let mut act = Active { stream: s.clone(), orig_sink: orig, procs: vec![], seen: Instant::now(), gone_since: None };
    let fail = |act: Active, msg: String| -> Result<Value, String> { undo(act); Err(msg) };

    let mut c = Command::new("pw-loopback"); c.args(loop_args());
    match spawn_child(c) { Ok(ch) => act.procs.push(ch), Err(e) => return fail(act, e) }
    let t0 = Instant::now();
    loop {
        if act.procs.iter_mut().any(|c| matches!(c.try_wait(), Ok(Some(_)))) { return fail(act, "pw-loopback stopped right away; PipeWire may not be running.".into()); }
        if list_sinks().is_ok_and(|l| l.iter().any(|(_, n)| n == SINK)) { break; }
        if t0.elapsed() > Duration::from_secs(5) { return fail(act, "The virtual sound device did not appear.".into()); }
        std::thread::sleep(Duration::from_millis(100));
    }
    let mut c = Command::new("pw-loopback"); c.args(listen_args(&prev));
    match spawn_child(c) { Ok(ch) => act.procs.push(ch), Err(e) => return fail(act, e) }
    if let Err(e) = run_capture("pactl", &move_args(id, SINK)) { return fail(act, format!("Could not take that app's sound: {e}")); }
    let after = defaults();
    if after != before {
        // Nothing of ours should ever become a default; if something did, put it back.
        if before.0 != after.0 && valid_name(&before.0) { let _ = pactl(&["set-default-sink", &before.0]); }
        if before.1 != after.1 && valid_name(&before.1) { let _ = pactl(&["set-default-source", &before.1]); }
    }
    *g = Some(act);
    drop(g);
    watchdog();
    Ok(json!({"label": LABEL, "app": s.app, "title": s.title, "id": id}))
}

pub fn status() -> Value {
    let (stream, orig) = {
        let mut g = ACTIVE.lock().unwrap();
        match g.as_mut() { Some(a) => { a.seen = Instant::now(); (a.stream.clone(), a.orig_sink) } , None => {
            return json!({"active": false, "reason": ENDED.lock().unwrap().clone().unwrap_or_default()});
        } }
    };
    let _ = orig;
    let now = list_streams().ok();
    let cur = now.as_ref().and_then(|l| l.iter().find(|s| s.id == stream.id && s.binary == stream.binary).cloned());
    let mut g = ACTIVE.lock().unwrap();
    let Some(a) = g.as_mut() else { return json!({"active": false, "reason": ENDED.lock().unwrap().clone().unwrap_or_default()}) };
    match (&cur, now.is_some()) {
        (Some(_), _) => a.gone_since = None,
        (None, true) => { a.gone_since.get_or_insert_with(Instant::now); }
        (None, false) => {}   // pactl hiccup: say nothing new
    }
    let gone = a.gone_since.map(|t| t.elapsed().as_secs()).unwrap_or(0);
    let c = cur.unwrap_or(stream);
    json!({"active": true, "alive": a.gone_since.is_none(), "gone_secs": gone, "app": c.app, "title": c.title, "corked": c.corked})
}

pub fn stop() -> Value {
    let a = ACTIVE.lock().unwrap().take();
    if let Some(a) = a { undo(a); }
    *ENDED.lock().unwrap() = None;
    json!({"active": false})
}
/// Called when the app shuts down.
pub fn shutdown() { let _ = stop(); }

pub fn rpc(method: &str, a: &Value) -> Result<Value, String> {
    match method {
        "pcaudio_caps" => Ok(caps()),
        "pcaudio_streams" => streams(),
        "pcaudio_start" => start(a),
        "pcaudio_status" => Ok(status()),
        "pcaudio_stop" => Ok(stop()),
        _ => Err("unknown method".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[
 {"index": 52, "driver": "PipeWire", "owner_module": "4294967295", "client": "88", "sink": 63, "corked": false, "mute": false,
  "properties": {"media.name": "Sea Power - Waving Flags", "application.name": "spotify", "application.process.id": "4242", "application.process.binary": "spotify"}},
 {"index": 77, "sink": 63, "corked": true, "properties": {"media.name": "Playback", "application.name": "Brave", "application.process.binary": "brave"}},
 {"index": 78, "sink": 63, "properties": {"media.name": "AudioStream", "application.name": "Firefox", "application.process.binary": "firefox"}},
 {"index": 79, "sink": 63, "properties": {"media.name": "Chromium", "application.name": "Chromium", "application.process.binary": "/usr/lib/chromium/chromium"}},
 {"index": 90, "sink": 64, "properties": {"media.name": "output", "application.name": "megatron_pcaudio_listen", "application.process.binary": "pw-loopback"}},
 {"index": 91, "sink": 63, "properties": {"media.name": "x\u0007y", "application.process.binary": "mpv"}},
 {"index": -3, "properties": {}}, {"index": 4294967296, "properties": {}}, {"properties": {"media.name": "no index"}}
]"#;

    #[test]
    fn parses_streams_and_skips_unusable_entries() {
        let s = parse_sink_inputs(SAMPLE).unwrap();
        assert_eq!(s.iter().map(|x| x.id).collect::<Vec<_>>(), vec![52, 77, 78, 79, 90, 91]);
        assert_eq!(s[0], Stream { id: 52, app: "spotify".into(), title: "Sea Power - Waving Flags".into(), binary: "spotify".into(), sink: Some(63), corked: false });
        assert!(s[1].corked);
        assert_eq!(s[5].app, "mpv", "falls back to the binary name");
        assert_eq!(s[5].title, "xy", "control characters are dropped");
        assert!(parse_sink_inputs("not json").is_err());
        assert!(parse_sink_inputs("{}").is_err());
        assert_eq!(parse_sink_inputs("[]").unwrap(), vec![]);
    }

    #[test]
    fn browsers_and_our_own_streams_are_never_offered() {
        let s = parse_sink_inputs(SAMPLE).unwrap();
        let offered: Vec<u32> = s.iter().filter(|x| refusal(x).is_none()).map(|x| x.id).collect();
        assert_eq!(offered, vec![52, 91]);
        assert!(is_browser(&s[1]) && is_browser(&s[2]) && is_browser(&s[3]));
        assert!(is_ours(&s[4]) && !is_browser(&s[4]));
        let mut z = s[0].clone(); z.app = "zenity".into(); z.binary = "zenity".into();
        assert!(refusal(&z).is_none(), "zenity is not the Zen browser");
        let mut e = s[0].clone(); e.app = "Google Chrome".into(); e.binary = "".into();
        assert!(refusal(&e).is_some());
    }

    #[test]
    fn parses_sinks() {
        let v = parse_sinks(r#"[{"index": 63, "name": "alsa_output.pci-0000_0b_00.4.analog-stereo"}, {"index": 64, "name": "megatron_pcaudio_sink"}, {"name": "no index"}]"#).unwrap();
        assert_eq!(v, vec![(63, "alsa_output.pci-0000_0b_00.4.analog-stereo".to_string()), (64, SINK.to_string())]);
    }

    #[test]
    fn ids_and_names_are_strictly_checked() {
        assert_eq!(parse_id(&json!(52)), Ok(52));
        for bad in [json!("52"), json!(-1), json!(1.5), json!(4294967296u64), json!(null), json!("52; rm -rf ~"), json!([1])] { assert!(parse_id(&bad).is_err(), "{bad}"); }
        assert!(valid_name("alsa_output.pci-0000_0b_00.4.analog-stereo") && valid_name("bluez_output.AA_BB.1"));
        for bad in ["", "-x", "a b", "a;b", "a\nb", "a\"b", "$(id)", "a`b"] { assert!(!valid_name(bad), "{bad:?}"); }
    }

    #[test]
    fn builds_exact_argument_lists() {
        let l = loop_args();
        assert_eq!(&l[..6], ["-n", LOOP_NAME, "-c", "2", "-m", "[ FL, FR ]"]);
        let (i, o) = (&l[7], &l[9]);
        assert!(i.contains("media.class=Audio/Sink") && i.contains("node.name=megatron_pcaudio_sink") && i.contains("node.dont-fallback=true") && i.contains("priority.session=1"));
        assert!(o.contains("media.class=Audio/Source") && o.contains("node.name=megatron_pcaudio_src") && o.contains("node.description=\"Megatron PC audio\"") && o.contains("node.dont-fallback=true"));
        for p in [i, o] { assert!(!p.contains("default"), "never mentions a default"); }
        let h = listen_args("alsa_output.x");
        assert_eq!(&h[..4], ["-n", LISTEN_NAME, "-c", "2"]);
        assert!(h.windows(2).any(|w| w == ["-C", SINK]) && h.windows(2).any(|w| w == ["-P", "alsa_output.x"]));
        assert!(h.iter().any(|a| a.contains("stream.capture.sink=true")));
        assert_eq!(move_args(52, SINK), ["move-sink-input", "52", "megatron_pcaudio_sink"]);
        assert_eq!(move_args(52, "63"), ["move-sink-input", "52", "63"]);
    }

    #[test]
    fn stale_matching_uses_exact_node_names() {
        let a = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(cmdline_is_ours(&a(&["pw-loopback", "-n", LOOP_NAME, "-c", "2"])));
        assert!(cmdline_is_ours(&a(&["/usr/bin/pw-loopback", "-n", LISTEN_NAME])));
        assert!(cmdline_is_ours(&a(&["/bin/sh", "/tmp/fake/pw-loopback", "-n", LOOP_NAME])));
        assert!(cmdline_is_ours(&a(&["pw-loopback", "--name=megatron_pcaudio_loop"])));
        assert!(!cmdline_is_ours(&a(&["pw-loopback", "-n", "megatron_pcaudio_loop2"])), "prefix is not enough");
        assert!(!cmdline_is_ours(&a(&["pw-loopback", "-n", "someone_elses"])));
        assert!(!cmdline_is_ours(&a(&["pw-loopback", "-i", "node.name=megatron_pcaudio_sink"])), "only -n counts");
        assert!(!cmdline_is_ours(&a(&["vim", "-n", LOOP_NAME])));
        assert!(!cmdline_is_ours(&a(&[])));
    }
}
