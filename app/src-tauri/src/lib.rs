// The UI is plain HTML/JS. It can only call `rpc`, which forwards to commons-core. No keys or plaintext
// are ever handed to JavaScript beyond what is displayed.
//
// This is a library so that the same code starts the app on the desktop (src/main.rs) and on iOS, where Tauri's
// mobile entry point takes the place of `main`.
use commons_core::api::App;
use serde_json::Value;
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use tauri::Manager;

#[cfg(desktop)]
mod update;

/// The page's timer-driven polls: the half-second `snapshot` and `messages`, and the live/ring polls that carry the
/// cards' presence. Their answers (a room's whole window, the whole app snapshot) are the largest JSON this process
/// builds, and they are built twice a second, so they run on one dedicated thread: glibc keeps a separate allocator
/// arena per thread, and building a big answer on a fresh pool thread each time left every one of those threads at
/// its high-water mark (~745 MB against ~320 MB when the half-second poll ran on one thread). The relay's
/// `call_presence` stays on the pool: it blocks on a relay request and must not hold the polls up behind it.
fn is_poll(method: &str) -> bool { matches!(method, "snapshot" | "messages" | "live_poll" | "ring_poll") }

/// The one worker thread that answers the polling calls above.
struct Poller { tx: Sender<(String, Value, Sender<Result<Value, String>>)> }

impl Poller {
    fn start(app: Arc<App>) -> Poller {
        let (tx, rx) = channel::<(String, Value, Sender<Result<Value, String>>)>();
        std::thread::Builder::new().name("rpc-poll".into()).spawn(move || {
            while let Ok((method, args, reply)) = rx.recv() { let _ = reply.send(app.call(&method, args)); }
        }).expect("spawn the poll worker");
        Poller { tx }
    }
    async fn call(&self, method: String, args: Value) -> Result<Value, String> {
        let (reply, rx) = channel();
        self.tx.send((method, args, reply)).map_err(|_| "poll worker stopped".to_string())?;
        // The answer is built on the poll thread; this only waits for it (nothing here is allocated on the pool).
        tauri::async_runtime::spawn_blocking(move || rx.recv().map_err(|_| "poll worker stopped".to_string())?)
            .await.map_err(|e| e.to_string())?
    }
}

#[tauri::command]
async fn rpc(_handle: tauri::AppHandle, app: tauri::State<'_, Arc<App>>, poller: tauri::State<'_, Poller>, method: String, args: Value) -> Result<Value, String> {
    // A phone has no desktop to hand a link to, so the system opens it directly in Safari.
    #[cfg(mobile)]
    if method == "open_link" {
        let url = args.get("url").and_then(|x| x.as_str()).unwrap_or("");
        if !commons_core::api::link_ok(url) { return Err("that is not a web address this app opens".into()); }
        tauri_plugin_opener::OpenerExt::opener(&_handle).open_url(url, None::<&str>).map_err(|e| e.to_string())?;
        return Ok(serde_json::json!({"opened": true}));
    }
    if is_poll(&method) { return poller.call(method, args).await; }
    let app = app.inner().clone();
    tauri::async_runtime::spawn_blocking(move || app.call(&method, args)).await.map_err(|e| e.to_string())?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // The updater's HTTP client (reqwest with rustls) needs a process-wide crypto provider before its first download.
    // There is no updater on a phone, and no rustls either.
    #[cfg(desktop)]
    let _ = rustls::crypto::ring::default_provider().install_default();

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();

    #[cfg(mobile)]
    { builder = builder.plugin(tauri_plugin_opener::init()); }

    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build()).manage(update::Updates::default());
        // Opening the app twice brings the window forward; two copies on one account would reuse keys. With COMMONS_DATA
        // set, several instances on separate folders are the point, and the core's lock on each folder still guards every case.
        if std::env::var_os("COMMONS_DATA").is_none() {
            builder = builder.plugin(tauri_plugin_single_instance::init(|app, _, _| {
                if let Some(w) = app.get_webview_window("main") { let _ = w.unminimize(); let _ = w.show(); let _ = w.set_focus(); }
            }));
        }
    }

    #[cfg(desktop)]
    let builder = builder.invoke_handler(tauri::generate_handler![rpc, update::update_check, update::update_install, update::update_apply, update::update_status]);
    #[cfg(mobile)]
    let builder = builder.invoke_handler(tauri::generate_handler![rpc]);

    builder
        .setup(|t| {
            // COMMONS_DATA lets you run several independent instances ("two windows") on one machine.
            let dir = std::env::var("COMMONS_DATA").map(std::path::PathBuf::from).unwrap_or_else(|_| t.path().app_data_dir().expect("app data dir"));
            // The server this build belongs to (default-relay.txt), so signing in on a new device or with a recovery key
            // needs no invite link. COMMONS_RELAY still overrides it; an empty file means no default.
            let relay = std::env::var("COMMONS_RELAY").unwrap_or_else(|_| include_str!("../default-relay.txt").trim().to_string());
            let app = App::new(dir, &relay);
            app.start_background();
            let poller = Poller::start(app.clone());
            // The same notifier as the browser version's background part: a new message while this window is hidden or not
            // focused is shown by the system (the page reports that through its heartbeat). Phones have their own path.
            #[cfg(desktop)]
            {
                let hub = app.notify_hub();
                hub.set_sink(Arc::new(commons_core::notify::NativeSink::new(&hub, None, "app.commons.chat")));
            }
            t.manage(app);
            t.manage(poller);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Commons")
        .run(|handle, event| {
            // Stop the sync loop (and the browser-call listener) when the app quits.
            if let tauri::RunEvent::Exit = event {
                if let Some(app) = handle.try_state::<Arc<App>>() { app.shutdown(); }
            }
        });
}
