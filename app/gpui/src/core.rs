//! The core, linked directly. Every call goes through `App::call(method, args)`, the same JSON API the Tauri shell and
//! the browser bridge use, so this app cannot drift from what iOS and the browser edition do.
use commons_core::api::App;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

pub struct Core {
    pub app: Arc<App>,
    pub dir: PathBuf,
    /// The half-second look runs on one thread of its own. A look at a long room builds thousands of small JSON values;
    /// spread over a pool of threads, each thread's allocator kept its own high-water mark (measured: 745 MB with 5000
    /// rows open, see SPIKE.md). One thread keeps one.
    /// What desktop notifications ask of the window (a click, a quick reply that was sent); read by the window's clock.
    pub inbox: Arc<crate::notifier::Inbox>,
    /// The notification service with quick reply, where there is one (Linux); also used for rings and missed calls.
    pub notifier: Option<Arc<crate::notifier::Notifier>>,
    looks: std::sync::Mutex<std::sync::mpsc::Sender<Box<dyn FnOnce(&App) + Send>>>,
}

/// Where the account lives: the same folder as the Tauri app (`app.commons.chat`), so this app opens the account that
/// is already there. `COMMONS_DATA` overrides it (tests, a second profile), as in the Tauri app. The core's folder lock
/// (client.rs `lock_dir`, taken when the account is unlocked or created) keeps the two programs off one account at the
/// same time: the second one gets "this account is already open in another Megatron window or process". Nothing here
/// reads or rewrites the folder before that unlock.
pub fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("COMMONS_DATA") { return PathBuf::from(d); }
    let base = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| {
            let h = PathBuf::from(h);
            if cfg!(target_os = "macos") { h.join("Library/Application Support") } else { h.join(".local/share") }
        }))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("app.commons.chat")
}

/// True when another program (the older Tauri app, or another mega) holds the account folder's lock right now. Looks
/// only: the lock is taken and dropped at once, and nothing is created when the folder or its lock file is missing.
pub fn profile_busy(dir: &std::path::Path) -> bool {
    let Ok(f) = std::fs::OpenOptions::new().write(true).open(dir.join("lock")) else { return false };
    matches!(f.try_lock(), Err(std::fs::TryLockError::WouldBlock))
}
pub const BUSY_TEXT: &str = "mega is already open on this account (the older mega app or another window). Close it first, then try again.";

/// The core's refusal when the lock is taken, in words that say what to do.
pub fn friendly_error(e: &str) -> String {
    if e.contains("already open in another") { BUSY_TEXT.to_string() } else { e.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn busy_when_lock_held_and_never_creates() {
        let d = std::env::temp_dir().join(format!("mega-busy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        assert!(!profile_busy(&d));
        assert!(!d.exists(), "looking must not create the folder");
        std::fs::create_dir_all(&d).unwrap();
        assert!(!profile_busy(&d) && !d.join("lock").exists());
        let held = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(d.join("lock")).unwrap();
        held.try_lock().unwrap();
        assert!(profile_busy(&d));
        drop(held);
        assert!(!profile_busy(&d));
        assert_eq!(friendly_error("this account is already open in another Megatron window or process; close that one first"), BUSY_TEXT);
        let _ = std::fs::remove_dir_all(&d);
    }
}

impl Core {
    pub fn open() -> Arc<Core> {
        let dir = data_dir();
        let relay = std::env::var("COMMONS_RELAY").unwrap_or_else(|_| include_str!("../../src-tauri/default-relay.txt").trim().to_string());
        // Tests only (never set for a person's app): the cheap key derivation, so a password invite made here and one looked up by a test bridge agree.
        let kdf = if std::env::var_os("COMMONS_FAST_KDF").is_some() { commons_core::vault::KdfParams::TEST } else { commons_core::vault::KdfParams::STRONG };
        let app = App::with_kdf(dir.clone(), &relay, kdf);
        app.start_background();
        let hub = app.notify_hub();
        // Notes go to the system exactly as in the Tauri app. A check that must not show anything writes them to a file instead.
        let inbox = Arc::new(crate::notifier::Inbox::default());
        let mut notifier = None;
        match std::env::var_os("MEGA_GPUI_NOTIFY_FILE") {
            Some(f) => hub.set_sink(Arc::new(commons_core::notify::FileSink::new(PathBuf::from(f)))),
            None => {
                #[cfg(all(feature = "calls", target_os = "linux"))]
                {
                    // Linux: the freedesktop service directly, with a quick reply where the desktop offers one (notifier.rs)
                    let weak = Arc::downgrade(&app);
                    let reply: crate::notifier::Reply = Arc::new(move |gid, text| {
                        let app = weak.upgrade().ok_or("closing")?;
                        app.call("send_text", serde_json::json!({"gid": gid, "text": text})).map(|_| ())
                    });
                    let n = crate::notifier::Notifier::start(crate::notifier::Bus::from_env(), reply, inbox.clone());
                    hub.set_sink(n.clone());
                    notifier = Some(n);
                }
                #[cfg(target_os = "macos")]
                {
                    // macOS: UNUserNotificationCenter with a delegate, so a click opens the room (macnotify.rs)
                    let weak = Arc::downgrade(&app);
                    let reply: crate::notifier::Reply = Arc::new(move |gid, text| {
                        let app = weak.upgrade().ok_or("closing")?;
                        app.call("send_text", serde_json::json!({"gid": gid, "text": text})).map(|_| ())
                    });
                    let n = crate::notifier::Notifier::start(crate::notifier::Bus::from_env(), reply, inbox.clone());
                    hub.set_sink(n.clone());
                    // tests only: MEGA_GPUI_NOTIFY_SELFTEST=<room id> shows one note for that room a few seconds after start,
                    // to click by hand (or by script) and read "notification clicked: room <id>" in the output
                    if let Some(gid) = std::env::var("MEGA_GPUI_NOTIFY_SELFTEST").ok().filter(|g| !g.is_empty()) {
                        let n2 = n.clone();
                        std::thread::spawn(move || { std::thread::sleep(std::time::Duration::from_secs(5)); n2.show_plain("selftest", "mega self-test", "Click this note: it should open the room", Some(&gid)); eprintln!("notification self-test shown for room {gid}"); });
                    }
                    notifier = Some(n);
                }
                #[cfg(not(any(all(feature = "calls", target_os = "linux"), target_os = "macos")))]
                hub.set_sink(Arc::new(commons_core::notify::NativeSink::new(&hub, None, "app.commons.chat")));
            }
        }
        let (tx, rx) = std::sync::mpsc::channel::<Box<dyn FnOnce(&App) + Send>>();
        let worker = app.clone();
        std::thread::Builder::new().name("mega-look".into()).spawn(move || { while let Ok(job) = rx.recv() { job(&worker); } }).expect("thread");
        Arc::new(Core { app, dir, inbox, notifier, looks: std::sync::Mutex::new(tx) })
    }
    pub fn call(&self, method: &str, args: Value) -> Result<Value, String> { self.app.call(method, args) }
    /// Run `f` on the look thread and hand back what it made.
    pub fn look<T: Send + 'static>(&self, f: impl FnOnce(&App) -> T + Send + 'static) -> futures::channel::oneshot::Receiver<T> {
        let (tx, rx) = futures::channel::oneshot::channel();
        let _ = self.looks.lock().unwrap().send(Box::new(move |app| { let _ = tx.send(f(app)); }));
        rx
    }
}
