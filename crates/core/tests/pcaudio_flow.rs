//! The app-sound route end to end with a FAKE `pactl` and `pw-loopback` on PATH. Nothing here touches the real PipeWire:
//! the fakes keep their little world in a temp directory. One test function only, because PATH is process-wide.
use commons_core::pcaudio as pc;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const PACTL: &str = include_str!("../../../tools/fake-pw/pactl");
const LOOPBACK: &str = include_str!("../../../tools/fake-pw/pw-loopback");

fn script(dir: &Path, name: &str, body: &str) {
    let p = dir.join(name); fs::write(&p, body).unwrap();
    use std::os::unix::fs::PermissionsExt; fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
}
fn until(what: &str, mut f: impl FnMut() -> bool) { let t = Instant::now(); while !f() { assert!(t.elapsed() < Duration::from_secs(10), "timeout: {what}"); std::thread::sleep(Duration::from_millis(50)); } }
fn lines(p: &Path) -> Vec<String> { fs::read_to_string(p).unwrap_or_default().lines().map(String::from).collect() }
fn alive(pid: &str) -> bool { Path::new(&format!("/proc/{pid}")).exists() && fs::read_to_string(format!("/proc/{pid}/stat")).map(|s| !s.contains(") Z")).unwrap_or(false) }

struct Reap(Vec<std::process::Child>);
impl Drop for Reap { fn drop(&mut self) { for c in &mut self.0 { let _ = c.kill(); let _ = c.wait(); } } }

#[test]
fn app_sound_route_with_fake_tools() {
    let dir: PathBuf = std::env::temp_dir().join(format!("pcaudio-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let (bin, st) = (dir.join("bin"), dir.join("state"));
    fs::create_dir_all(&bin).unwrap(); fs::create_dir_all(&st).unwrap();
    script(&bin, "pactl", PACTL); script(&bin, "pw-loopback", LOOPBACK);
    fs::write(st.join("ids"), "52 77 90").unwrap();
    std::env::set_var("FAKE_STATE", &st);
    std::env::set_var("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()));

    assert_eq!(pc::rpc("pcaudio_caps", &json!({})).unwrap()["supported"], true);

    // the list: Spotify only. The browser and our own loopback stream are hidden.
    let l = pc::rpc("pcaudio_streams", &json!({})).unwrap();
    let ids: Vec<u64> = l["streams"].as_array().unwrap().iter().map(|s| s["id"].as_u64().unwrap()).collect();
    assert_eq!(ids, vec![52]);
    assert_eq!(l["browser_hidden"], 1);

    // refusals: browser stream, bad ids, missing stream. Nothing may have been started or moved.
    for bad in [json!({"id": 77}), json!({"id": 90}), json!({"id": "52"}), json!({"id": -1}), json!({"id": 1.5}), json!({}), json!({"id": 4294967296u64}), json!({"id": 99})] {
        assert!(pc::rpc("pcaudio_start", &bad).is_err(), "{bad}");
    }
    assert!(lines(&st.join("loopbacks.log")).is_empty() && lines(&st.join("moves.log")).is_empty());

    // start
    let r = pc::rpc("pcaudio_start", &json!({"id": 52})).unwrap();
    assert_eq!(r["label"], "Megatron PC audio");
    let lb = lines(&st.join("loopbacks.log"));
    assert_eq!(lb.len(), 2, "{lb:?}");
    assert!(lb[0].starts_with("megatron_pcaudio_loop ") && lb[1].starts_with("megatron_pcaudio_listen "));
    assert_eq!(lines(&st.join("moves.log")), vec!["52 64"]);
    assert!(lines(&st.join("defaults.log")).is_empty(), "no default was touched");
    assert!(lines(&st.join("unexpected.log")).is_empty());
    // a second start is refused, and the list now shows the stream on our sink (still only the app)
    assert!(pc::rpc("pcaudio_start", &json!({"id": 52})).is_err());
    let s = pc::rpc("pcaudio_status", &json!({})).unwrap();
    assert_eq!((s["active"].clone(), s["alive"].clone(), s["title"].clone()), (json!(true), json!(true), json!("Sea Power - Waving Flags")));

    // the app's stream ends: reported, not undone by the core (the page decides)
    fs::write(st.join("ids"), "77 90").unwrap();
    let s = pc::rpc("pcaudio_status", &json!({})).unwrap();
    assert_eq!(s["alive"], false);
    fs::write(st.join("ids"), "52 77 90").unwrap();
    assert_eq!(pc::rpc("pcaudio_status", &json!({})).unwrap()["alive"], true);

    // stop: the stream goes back to its own sink, both loopbacks end, nothing is left
    let pids: Vec<String> = lines(&st.join("loopbacks.log")).iter().map(|l| l.split(' ').nth(1).unwrap().to_string()).collect();
    pc::rpc("pcaudio_stop", &json!({})).unwrap();
    assert_eq!(lines(&st.join("moves.log")), vec!["52 64", "52 63"]);
    until("loopbacks gone", || pids.iter().all(|p| !alive(p)));
    assert_eq!(pc::rpc("pcaudio_status", &json!({})).unwrap()["active"], false);
    assert_eq!(pc::rpc("pcaudio_streams", &json!({})).unwrap()["streams"][0]["id"], 52);
    pc::rpc("pcaudio_stop", &json!({})).unwrap();   // twice is fine
    assert_eq!(lines(&st.join("moves.log")).len(), 2);

    // a loopback of ours left by a crashed bridge is ended by the next start; somebody else's is not
    fs::remove_file(st.join("sink_of_52")).ok();
    let mut reap = Reap(vec![]);
    reap.0.push(std::process::Command::new("pw-loopback").args(["-n", "megatron_pcaudio_loop"]).spawn().unwrap());
    reap.0.push(std::process::Command::new("pw-loopback").args(["-n", "megatron_pcaudio_loop_not_ours"]).spawn().unwrap());
    until("fakes started", || lines(&st.join("loopbacks.log")).len() >= 4);
    fs::remove_file(st.join("loop.pid")).ok();
    let before = lines(&st.join("moves.log")).len();
    pc::rpc("pcaudio_start", &json!({"id": 52})).unwrap();
    until("stale loopback ended", || reap.0[0].try_wait().unwrap().is_some());
    assert!(reap.0[1].try_wait().unwrap().is_none(), "a pw-loopback with another name is left alone");
    assert_eq!(lines(&st.join("moves.log")).len(), before + 1);

    // a loopback dying under us (the sound server went away): the watchdog moves the stream back and reports why
    let pid = fs::read_to_string(st.join("loop.pid")).unwrap();
    std::process::Command::new("kill").args(["-KILL", pid.trim()]).status().unwrap();
    until("watchdog undid it", || pc::rpc("pcaudio_status", &json!({})).unwrap()["active"] == false);
    let s = pc::rpc("pcaudio_status", &json!({})).unwrap();
    assert!(s["reason"].as_str().unwrap().contains("stopped unexpectedly"), "{s}");
    until("stream moved back", || lines(&st.join("moves.log")).last().map(|l| l == "52 63").unwrap_or(false));

    let _ = fs::remove_dir_all(&dir);
}
