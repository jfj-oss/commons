//! Signed updates (docs/UPDATES.md). The manifest comes from the relay this account uses, at `<relay>/update/latest.json`,
//! and is checked by `commons_core::update` (same server only, no redirects). The file is downloaded and checked by
//! Tauri's updater plugin: its minisign signature against the public key in tauri.conf.json, made for exactly the
//! version the manifest announces. Nothing is installed without the person saying so, and never while a call runs.
//!
//! The UI drives it: `update_check` (on start and every few hours), then `update_install` after "Install now", then
//! `update_apply` once no call is running. `update_status` reports progress.
use commons_core::api::App;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, State};
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Default)]
pub struct Updates {
    /// The update the person was offered (the one "Install now" means).
    offered: Mutex<Option<Update>>,
    /// Downloaded and verified, waiting for a call to end.
    ready: Mutex<Option<(Update, Vec<u8>)>>,
    busy: AtomicBool,
    got: AtomicU64,
    total: AtomicU64,
}

fn relay_of(core: &App) -> Option<String> {
    let st = core.call("state", json!({})).ok()?;
    st.get("relay").and_then(|r| r.as_str()).filter(|r| !r.is_empty()).map(String::from)
}

/// What a person can make of an updater error. The plugin's own words stay at the end, for whoever helps them.
fn explain(e: tauri_plugin_updater::Error) -> String {
    use tauri_plugin_updater::Error as E;
    let why = e.to_string();
    let head = match &e {
        E::Minisign(_) | E::SignatureUtf8(_) | E::Base64(_) => "The downloaded update did not pass its signature check, so it was NOT installed. It may have been damaged or tampered with on the way.",
        E::SignedVersionMismatch { .. } | E::MissingSignedVersion => "The update file is signed for a different version than the server announced, so it was NOT installed.",
        E::Network(_) | E::Reqwest(_) => "The update could not be downloaded. Try again later.",
        _ => "The update could not be installed.",
    };
    format!("{head} ({why})")
}

fn updater(handle: &AppHandle, manifest_url: &str) -> Result<tauri_plugin_updater::Updater, String> {
    let url = manifest_url.parse().map_err(|e| format!("bad update address: {e}"))?;
    handle.updater_builder()
        .endpoints(vec![url]).map_err(|e| e.to_string())?
        // The file must come from where the manifest says, on the relay: never follow a redirect elsewhere.
        .configure_client(|b| b.redirect(reqwest::redirect::Policy::none()))
        .timeout(std::time::Duration::from_secs(600))
        .build().map_err(|e| e.to_string())
}

/// Is there a newer version on this account's relay? Nothing is downloaded here.
#[tauri::command]
pub async fn update_check(handle: AppHandle, core: State<'_, Arc<App>>, st: State<'_, Updates>) -> Result<Value, String> {
    let core = core.inner().clone();
    let current = handle.package_info().version.to_string();
    let Some(relay) = relay_of(&core) else { return Ok(json!({"available": false, "current": current, "why": "locked"})) };
    let fetched = tauri::async_runtime::spawn_blocking(move || commons_core::update::fetch_manifest(&relay).map(|m| (m, relay)))
        .await.map_err(|e| e.to_string())??;
    let (Some(manifest), relay) = fetched else { return Ok(json!({"available": false, "current": current, "why": "this server publishes no updates"})) };
    let required = commons_core::update::below_minimum(&current, &manifest);
    let minimum = commons_core::update::minimum(&manifest);
    let up = updater(&handle, &commons_core::update::manifest_url(&relay)?)?;
    // restore_update: the plugin builds the update from the manifest we fetched and checked, and insists that the
    // signature names the announced version.
    let update = match up.restore_update(manifest) {
        Ok(u) => u,
        Err(tauri_plugin_updater::Error::TargetsNotFound(_) | tauri_plugin_updater::Error::TargetNotFound(_)) =>
            return Ok(json!({"available": false, "current": current, "required": required, "minimum": minimum, "why": "no update for this system yet"})),
        Err(e) => return Err(e.to_string()),
    };
    let out = match &update {
        Some(u) => json!({"available": true, "version": u.version, "current": current, "notes": u.body, "required": required, "minimum": minimum}),
        None => json!({"available": false, "current": current, "required": required, "minimum": minimum}),
    };
    *st.offered.lock().unwrap() = update;
    Ok(out)
}

/// "Install now": download and verify. If a call is running, or starts meanwhile, it waits for `update_apply`.
#[tauri::command]
pub async fn update_install(handle: AppHandle, core: State<'_, Arc<App>>, st: State<'_, Updates>) -> Result<Value, String> {
    if core.call_active() { return Ok(json!({"waiting_for_call": true})); }
    let Some(update) = st.offered.lock().unwrap().clone() else { return Err("There is no update to install. Check again later.".into()) };
    if st.busy.swap(true, Ordering::SeqCst) { return Ok(json!({"busy": true})); }
    st.got.store(0, Ordering::SeqCst); st.total.store(0, Ordering::SeqCst);
    let (got, total) = (&st.got, &st.total);
    let r = update.download(|n, len| { got.fetch_add(n as u64, Ordering::Relaxed); if let Some(l) = len { total.store(l, Ordering::Relaxed); } }, || {}).await;
    st.busy.store(false, Ordering::SeqCst);
    let bytes = r.map_err(explain)?;
    *st.ready.lock().unwrap() = Some((update, bytes));
    if core.call_active() { return Ok(json!({"ready": true, "waiting_for_call": true})); }
    apply(&handle, &core, &st)
}

/// Install what was downloaded and restart, unless a call is running.
#[tauri::command]
pub async fn update_apply(handle: AppHandle, core: State<'_, Arc<App>>, st: State<'_, Updates>) -> Result<Value, String> {
    apply(&handle, &core, &st)
}

fn apply(handle: &AppHandle, core: &App, st: &Updates) -> Result<Value, String> {
    if core.call_active() { return Ok(json!({"ready": true, "waiting_for_call": true})); }
    let Some((update, bytes)) = st.ready.lock().unwrap().take() else { return Err("Nothing has been downloaded yet.".into()) };
    // install() checks the signature again before it touches the installed app. On Windows it hands over to the
    // installer and the app quits; elsewhere the new version is in place and the app starts again.
    update.install(&bytes).map_err(explain)?;
    let h = handle.clone();
    std::thread::spawn(move || { std::thread::sleep(std::time::Duration::from_millis(300)); h.restart(); });
    Ok(json!({"restarting": true, "version": update.version}))
}

#[tauri::command]
pub fn update_status(handle: AppHandle, core: State<'_, Arc<App>>, st: State<'_, Updates>) -> Value {
    json!({"current": handle.package_info().version.to_string(), "in_call": core.call_active(), "downloading": st.busy.load(Ordering::SeqCst),
           "got": st.got.load(Ordering::Relaxed), "total": st.total.load(Ordering::Relaxed), "ready": st.ready.lock().unwrap().is_some()})
}
