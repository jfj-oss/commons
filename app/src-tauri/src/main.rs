// The UI is plain HTML/JS. It can only call `rpc`, which forwards to commons-core. No keys or plaintext
// are ever handed to JavaScript beyond what is displayed.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use commons_core::api::App;
use std::sync::Arc;
use tauri::Manager;

#[tauri::command]
async fn rpc(app: tauri::State<'_, Arc<App>>, method: String, args: serde_json::Value) -> Result<serde_json::Value, String> {
    let app = app.inner().clone();
    tauri::async_runtime::spawn_blocking(move || app.call(&method, args)).await.map_err(|e| e.to_string())?
}

fn main() {
    let mut builder = tauri::Builder::default();
    // Opening the app twice brings the window forward; two copies on one account would reuse keys. With COMMONS_DATA set,
    // several instances on separate folders are the point, and the core's lock on each folder still guards every case.
    if std::env::var_os("COMMONS_DATA").is_none() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(w) = app.get_webview_window("main") { let _ = w.unminimize(); let _ = w.show(); let _ = w.set_focus(); }
        }));
    }
    builder
        .setup(|t| {
            // COMMONS_DATA lets you run several independent instances ("two windows") on one machine.
            let dir = std::env::var("COMMONS_DATA").map(std::path::PathBuf::from).unwrap_or_else(|_| t.path().app_data_dir().expect("app data dir"));
            // No default: a relay is something a friend's invite brings along.
            let relay = std::env::var("COMMONS_RELAY").unwrap_or_default();
            let app = App::new(dir, &relay);
            app.start_background();
            t.manage(app);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![rpc])
        .run(tauri::generate_context!())
        .expect("error while running Commons");
}
