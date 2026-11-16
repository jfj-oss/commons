//! `mega-gpui --call-headless`: the call engine with the core and no window, for tools/gpui-call-e2e.mjs. Unlocks a
//! THROWAWAY account with `MEGA_GPUI_PASS`, needs `MEGA_GPUI_CALL_TEST=<port>` for its control port, and should run
//! with `MEGA_GPUI_CALL_AUDIO=none` so no audio device is opened. Prints one `ready` line, then serves until `quit`.
use super::engine::{AudioMode, CallEngine, CoreLink, Options};
use super::settings::CallSettings;
use super::testhook;
use crate::core::Core;
use std::sync::Arc;

struct Linked(Arc<Core>);
impl CoreLink for Linked {
    fn call(&self, method: &str, args: serde_json::Value) -> Result<serde_json::Value, String> { self.0.call(method, args) }
    fn call_active(&self) -> bool { self.0.app.call_active() }
}

/// Tests never touch the user's portal: off unless a test points `MEGA_GPUI_SHORTCUTS=bus:<address>` at its own bus.
fn headless_shortcuts() -> super::shortcuts::Mode {
    if std::env::var_os("MEGA_GPUI_SHORTCUTS").is_some() { super::shortcuts::Mode::from_env() } else { super::shortcuts::Mode::Off }
}

pub fn wanted() -> bool { std::env::args().any(|a| a == "--call-headless") }

pub fn run() {
    let Some(port) = testhook::port() else { eprintln!("--call-headless needs MEGA_GPUI_CALL_TEST=<port>"); std::process::exit(2) };
    let Ok(pass) = std::env::var("MEGA_GPUI_PASS") else { eprintln!("--call-headless needs MEGA_GPUI_PASS (a throwaway account)"); std::process::exit(2) };
    // no system notifications from a test run, ever
    if std::env::var_os("MEGA_GPUI_NOTIFY_FILE").is_none() && std::env::var_os("MEGA_GPUI_NOTIFY_BUS").is_none() { std::env::set_var("MEGA_GPUI_NOTIFY_FILE", crate::core::data_dir().join("notes.jsonl")); }
    let core = Core::open();
    if let Err(e) = core.call("unlock", serde_json::json!({"passphrase": pass})) { eprintln!("unlock: {e}"); std::process::exit(1); }
    let settings_path = core.dir.join("gpui-call-prefs.json");
    let settings = CallSettings::load(&settings_path);
    let engine = Arc::new(CallEngine::start(Arc::new(Linked(core.clone())), Options { audio: AudioMode::from_env(), settings, settings_path: Some(settings_path), shortcuts: headless_shortcuts() }));
    let rt = engine.rt.clone();
    let e2 = engine.clone();
    let me = core.call("snapshot", serde_json::json!({})).ok().map(|s| s["me"].clone()).unwrap_or_default();
    println!("{}", serde_json::json!({"ready": true, "port": port, "device": me["device"], "account": me["account"]}));
    if let Err(e) = rt.block_on(testhook::serve(e2, port)) { eprintln!("test port: {e}"); }
    // let the leave go out before the core stops
    std::thread::sleep(std::time::Duration::from_millis(800));
    core.app.shutdown();
}
