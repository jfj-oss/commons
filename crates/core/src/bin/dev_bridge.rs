//! Development bridge: serves the UI over http://127.0.0.1 and exposes the same `rpc` the Tauri shell has.
//! Used for headless UI checks and for trying the app in two browser tabs without Tauri. Loopback only, NO authentication.
//!   COMMONS_DATA=/tmp/alice COMMONS_RELAY=http://127.0.0.1:8787 dev_bridge 7001 ../../app/ui
//! A small thread-per-connection HTTP/1.1 server (every response closes its connection). It replaced tiny_http, whose
//! fixed worker pool made a browser's parallel script downloads (the 600 KB call library) stall for seconds or forever.
use commons_core::api::{bridge_request_ok, App};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// One Update click at a time.
static INSTALLING: AtomicBool = AtomicBool::new(false);

// Same policy as the desktop shell, widened only for what calls need: the LiveKit signalling socket, the frame-encryption worker, media streams.
// http:/https: on img-src, connect-src and media-src are for the owner's Jellyfin server, configured per device at an arbitrary
// address (often plain http on the LAN or Tailscale). A CSP cannot name a runtime-configured host, and Jellyfin media must be
// fetched and played by <video> directly (never proxied through the relay), so the widest part of the policy is limited to those
// three destinations. Scripts, workers and styles stay 'self'.
const CSP: &str = "default-src 'self'; img-src 'self' data: blob: http: https:; style-src 'self' 'unsafe-inline'; script-src 'self'; connect-src 'self' ws: wss: http: https:; worker-src 'self' blob:; media-src 'self' blob: mediastream: http: https:; base-uri 'none'; form-action 'none'; object-src 'none'";

fn reply(s: &mut TcpStream, code: u16, ct: &str, body: &[u8]) {
    let reason = match code { 200 => "OK", 403 => "Forbidden", 400 => "Bad Request", 404 => "Not Found", _ => "Error" };
    let head = format!("HTTP/1.1 {code} {reason}\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nContent-Security-Policy: {CSP}\r\n\r\n", body.len());
    let _ = s.write_all(head.as_bytes()).and_then(|_| s.write_all(body));
    let _ = s.flush();
}

/// The install folder and the update key, when this is an installed browser version. A bridge run from a source tree
/// has no key or no `bridge` beside its ui/, so it never updates anything.
fn updatable(ui: &std::path::Path) -> Option<(PathBuf, &'static str)> {
    use commons_core::browser_update::{bridge_name, public_key};
    let install = ui.parent()?;
    if !install.join(bridge_name()).is_file() { return None; }
    Some((install.to_path_buf(), public_key()?))
}

/// Checks, verifies and installs, or says why not. The caller restarts when `restarting` is true.
fn update_install(app: &App, install: &std::path::Path, key: &str) -> serde_json::Value {
    use commons_core::browser_update::{clear_note, run_now, version, Outcome};
    use serde_json::json;
    if app.call_active() { return json!({"in_call": true}); }
    if INSTALLING.swap(true, Ordering::SeqCst) { return json!({"busy": true}); }
    let relay = std::env::var("COMMONS_RELAY").unwrap_or_default();
    let out = match run_now(&relay, install, version(), key) {
        Ok(Outcome::Updated { version }) => return json!({"restarting": true, "version": version}),   // INSTALLING stays set
        Ok(Outcome::None) => json!({"current": true}),
        Ok(Outcome::Available { reason, .. }) => { clear_note(install); json!({"error": reason}) }
        Err(e) => json!({"error": e}),
    };
    INSTALLING.store(false, Ordering::SeqCst);
    out
}

/// Replaces this process with the bridge that was just installed. The account and the port are released first (every
/// descriptor is close-on-exec), and the launcher's pid stays right. If the new one cannot start, the previous copy
/// comes back here or at the next launch.
#[cfg(not(windows))]
fn restart(install: &std::path::Path, ui: &std::path::Path, port: u16) -> ! {
    use commons_core::browser_update::{bridge_name, restore_previous};
    use std::os::unix::process::CommandExt;
    #[cfg(target_os = "macos")]
    commons_core::browser_update::resign_bundle(install);
    let err = std::process::Command::new(install.join(bridge_name())).arg(port.to_string()).arg(ui).exec();
    eprintln!("could not start the new version: {err}");
    restore_previous(install);
    let err = std::process::Command::new(install.join(bridge_name())).arg(port.to_string()).arg(ui).exec();
    eprintln!("could not start the previous version: {err}");
    std::process::exit(1)
}

/// Windows has no exec: start the new bridge (it waits for the port) and leave. The launcher's pid file follows it.
#[cfg(windows)]
fn restart(install: &std::path::Path, ui: &std::path::Path, port: u16) -> ! {
    use commons_core::browser_update::{bridge_name, restore_previous};
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const FLAGS: u32 = 0x0000_0008 | 0x0800_0000 | 0x0000_0200;   // DETACHED_PROCESS | CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
    let start = || Command::new(install.join(bridge_name())).arg(port.to_string()).arg(ui)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(FLAGS).spawn();
    let child = start().or_else(|e| { eprintln!("could not start the new version: {e}"); restore_previous(install); start() });
    if let (Ok(c), Some(dir)) = (&child, std::env::var_os("MEGATRON_NOTE_DIR")) {
        let _ = std::fs::write(PathBuf::from(dir).join(".pid"), c.id().to_string());
    }
    std::process::exit(if child.is_ok() { 0 } else { 1 })
}

fn handle(mut s: TcpStream, app: Arc<App>, ui: Arc<PathBuf>, port: u16) {
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(30)));
    let mut r = BufReader::new(s.try_clone().expect("clone"));
    let mut line = String::new();
    if r.read_line(&mut line).unwrap_or(0) == 0 { return; }
    let mut parts = line.split_whitespace();
    let (method, url) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
    let (mut len, mut host, mut origin) = (0usize, None::<String>, None::<String>);
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" || h == "\n" { break; }
        let low = h.to_ascii_lowercase();
        if let Some(v) = low.strip_prefix("content-length:") { len = v.trim().parse().unwrap_or(0); }
        if let Some(v) = low.strip_prefix("host:") { host = Some(v.trim().to_string()); }
        if let Some(v) = low.strip_prefix("origin:") { origin = Some(v.trim().to_string()); }
    }
    if !bridge_request_ok(host.as_deref(), origin.as_deref(), port) { return reply(&mut s, 403, "text/plain", b"forbidden"); }
    if method == "GET" && url == "/update-note" {
        // What the launcher's update check left for the page (packaging/linux-browser/megatron). Read once, then gone.
        let note = ui.parent().and_then(commons_core::browser_update::take_note);
        let body = note.map(|n| n.to_string()).unwrap_or_else(|| "{}".into());
        return reply(&mut s, 200, "application/json", body.as_bytes());
    }
    if method == "GET" && url == "/update-check" {
        let body = match updatable(&ui) {
            None => serde_json::json!({"packaged": false}),
            Some(_) => {
                let relay = std::env::var("COMMONS_RELAY").unwrap_or_default();
                match commons_core::browser_update::check(&relay, commons_core::browser_update::version()) {
                    Ok(v) => v,
                    Err(e) => serde_json::json!({"error": e}),
                }
            }
        };
        return reply(&mut s, 200, "application/json", body.to_string().as_bytes());
    }
    if method == "POST" && url == "/update-install" {
        let Some((install, key)) = updatable(&ui) else {
            return reply(&mut s, 200, "application/json", br#"{"packaged":false}"#);
        };
        let out = update_install(&app, &install, key);
        reply(&mut s, 200, "application/json", out.to_string().as_bytes());
        if out["restarting"] == true {
            let _ = s.shutdown(std::net::Shutdown::Both);
            restart(&install, &ui, port);
        }
        return;
    }
    if method == "POST" && url == "/rpc" {
        let mut body = vec![0u8; len.min(40 << 20)];
        if r.read_exact(&mut body).is_err() { return reply(&mut s, 400, "text/plain", b"bad body"); }
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        let m = v["method"].as_str().unwrap_or("").to_string();
        let out = match app.call(&m, v["args"].clone()) { Ok(x) => serde_json::json!({"ok": x}), Err(e) => serde_json::json!({"err": e}) };
        return reply(&mut s, 200, "application/json", out.to_string().as_bytes());
    }
    let path = if url == "/" { "index.html".to_string() } else { url.trim_start_matches('/').split('?').next().unwrap_or("").to_string() };
    if path.contains("..") { return reply(&mut s, 400, "text/plain", b"bad path"); }
    match std::fs::read(ui.join(&path)) {
        Ok(b) => {
            let ct = if path.ends_with(".css") { "text/css" } else if path.ends_with(".js") { "text/javascript" } else if path.ends_with(".svg") { "image/svg+xml" } else { "text/html; charset=utf-8" };
            reply(&mut s, 200, ct, &b)
        }
        Err(_) => reply(&mut s, 404, "text/plain", b"not found"),
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let first = args.next();
    if first.as_deref() == Some("--update") {
        // The browser version's launcher runs this before starting the page (packaging/linux-browser/megatron):
        // it checks, verifies, installs, and leaves a note for the page. It never starts the server or opens the account.
        let (Some(relay), Some(install)) = (args.next(), args.next()) else {
            eprintln!("usage: bridge --update <relay> <install-dir>");
            std::process::exit(2);
        };
        let install = PathBuf::from(install);
        let out = commons_core::browser_update::run(
            &relay,
            &install,
            commons_core::browser_update::version(),
            commons_core::browser_update::public_key().unwrap_or(""),
        );
        match out {
            Ok(o) => println!("{}", commons_core::browser_update::describe(&o)),
            Err(e) => {
                eprintln!("update check failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    let port: u16 = first.and_then(|p| p.parse().ok()).unwrap_or(7001);
    let ui = Arc::new(PathBuf::from(args.next().unwrap_or_else(|| "app/ui".into())));
    let data = std::env::var("COMMONS_DATA").unwrap_or_else(|_| format!("./dev-data-{port}"));
    let relay = std::env::var("COMMONS_RELAY").unwrap_or_else(|_| "http://127.0.0.1:8787".into());
    let kdf = if std::env::var("COMMONS_FAST_KDF").is_ok() { commons_core::vault::KdfParams::TEST } else { commons_core::vault::KdfParams::STRONG };
    let app = App::with_kdf(data.into(), &relay, kdf);
    app.start_background();
    // After an Update on Windows the previous bridge may still be leaving, so give it a moment to free the port.
    let mut bound = TcpListener::bind(("127.0.0.1", port));
    for _ in 0..50 {
        if bound.is_ok() { break; }
        std::thread::sleep(std::time::Duration::from_millis(100));
        bound = TcpListener::bind(("127.0.0.1", port));
    }
    let listener = bound.expect("bind");
    // The port is ours: an update that was installed before this start left the previous copy in .update-previous.
    // It is no longer needed (rollback happens in the launcher only if the new bridge fails to start).
    if let Some(dir) = ui.parent() {
        commons_core::browser_update::cleanup_previous(dir);
    }
    eprintln!("dev bridge on http://127.0.0.1:{port}  (ui: {})", ui.display());
    for conn in listener.incoming().flatten() {
        let (app, ui) = (app.clone(), ui.clone());
        std::thread::spawn(move || handle(conn, app, ui, port));
    }
}
