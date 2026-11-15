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
    looks: std::sync::Mutex<std::sync::mpsc::Sender<Box<dyn FnOnce(&App) + Send>>>,
}

/// Where the account lives. `COMMONS_DATA` as in the Tauri app. Without it the spike uses a folder of its own, so it can
/// never touch the real one by accident; `MEGA_GPUI_REAL_PROFILE=1` opts into the Tauri app's folder (the core's lock on
/// the folder still keeps two programs from using one account at once).
pub fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("COMMONS_DATA") { return PathBuf::from(d); }
    let base = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| {
            let h = PathBuf::from(h);
            if cfg!(target_os = "macos") { h.join("Library/Application Support") } else { h.join(".local/share") }
        }))
        .unwrap_or_else(|| PathBuf::from("."));
    if std::env::var("MEGA_GPUI_REAL_PROFILE").as_deref() == Ok("1") { base.join("app.commons.chat") } else { base.join("app.commons.chat.gpui-spike") }
}

impl Core {
    pub fn open() -> Arc<Core> {
        let dir = data_dir();
        let relay = std::env::var("COMMONS_RELAY").unwrap_or_else(|_| include_str!("../../src-tauri/default-relay.txt").trim().to_string());
        let app = App::new(dir.clone(), &relay);
        app.start_background();
        let hub = app.notify_hub();
        // Notes go to the system exactly as in the Tauri app. A check that must not show anything writes them to a file instead.
        match std::env::var_os("MEGA_GPUI_NOTIFY_FILE") {
            Some(f) => hub.set_sink(Arc::new(commons_core::notify::FileSink::new(PathBuf::from(f)))),
            None => hub.set_sink(Arc::new(commons_core::notify::NativeSink::new(&hub, None, "app.commons.chat"))),
        }
        let (tx, rx) = std::sync::mpsc::channel::<Box<dyn FnOnce(&App) + Send>>();
        let worker = app.clone();
        std::thread::Builder::new().name("mega-look".into()).spawn(move || { while let Ok(job) = rx.recv() { job(&worker); } }).expect("thread");
        Arc::new(Core { app, dir, looks: std::sync::Mutex::new(tx) })
    }
    pub fn call(&self, method: &str, args: Value) -> Result<Value, String> { self.app.call(method, args) }
    /// Run `f` on the look thread and hand back what it made.
    pub fn look<T: Send + 'static>(&self, f: impl FnOnce(&App) -> T + Send + 'static) -> futures::channel::oneshot::Receiver<T> {
        let (tx, rx) = futures::channel::oneshot::channel();
        let _ = self.looks.lock().unwrap().send(Box::new(move |app| { let _ = tx.send(f(app)); }));
        rx
    }
}
