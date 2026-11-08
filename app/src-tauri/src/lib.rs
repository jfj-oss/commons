// The UI is plain HTML/JS. It can only call `rpc`, which forwards to commons-core. No keys or plaintext
// are ever handed to JavaScript beyond what is displayed.
//
// This is a library so that the same code starts the app on the desktop (src/main.rs) and on iOS, where Tauri's
// mobile entry point takes the place of `main`.
use commons_core::api::App;
use serde_json::Value;
use std::sync::Arc;
use tauri::Manager;

#[cfg(desktop)]
mod update;

#[tauri::command]
async fn rpc(_handle: tauri::AppHandle, app: tauri::State<'_, Arc<App>>, method: String, args: Value) -> Result<Value, String> {
    // A phone has no desktop to hand a link to, so the system opens it directly in Safari.
    #[cfg(mobile)]
    if method == "open_link" {
        let url = args.get("url").and_then(|x| x.as_str()).unwrap_or("");
        if !commons_core::api::link_ok(url) { return Err("that is not a web address this app opens".into()); }
        tauri_plugin_opener::OpenerExt::opener(&_handle).open_url(url, None::<&str>).map_err(|e| e.to_string())?;
        return Ok(serde_json::json!({"opened": true}));
    }
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
            t.manage(app);
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
