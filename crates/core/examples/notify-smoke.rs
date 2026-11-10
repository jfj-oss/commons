//! Real-path smoke test of the Linux notifier, for tools/notify-smoke.sh (run inside `dbus-run-session`, never on a real desktop):
//! shows one note through `NativeSink`, waits for the stub notification server to "click" it, and prints what happened.
use commons_core::notify::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn main() {
    let launcher = std::env::args().nth(1).map(std::path::PathBuf::from);
    let hub = Arc::new(Hub::new(None));
    hub.set_sink(Arc::new(NativeSink::new(&hub, launcher, "app.commons.chat.browser")));
    hub.set_show_text(std::env::var_os("SMOKE_TEXT").is_some());
    hub.deliver(vec![Ev { gid: "room-1".into(), room: "music".into(), who: "Ben".into(), text: "a <b>tag</b> & more".into(), file: false, why: Why::Plain }]);
    std::thread::sleep(Duration::from_secs(4));
    // the click started the launcher; the page that comes up is told which room to open
    println!("open after click: {}", hub.beat(true, true, Instant::now())["open"]);
}
