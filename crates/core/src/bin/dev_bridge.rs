//! Development bridge: serves the UI over http://127.0.0.1 and exposes the same `rpc` the Tauri shell has.
//! Used for headless UI checks, for trying the app in two browser tabs without Tauri, and (as `bridge`) by the browser
//! versions. Loopback only. With MEGATRON_SECRET set (the launchers do) every /rpc and /update-* call must present it
//! (commons_core::bridge_auth); without it the bridge is open to the Host/Origin rules only, as before.
//!   COMMONS_DATA=/tmp/alice COMMONS_RELAY=http://127.0.0.1:8787 dev_bridge 7001 ../../app/ui
//! A small thread-per-connection HTTP/1.1 server (every response closes its connection). It replaced tiny_http, whose
//! fixed worker pool made a browser's parallel script downloads (the 600 KB call library) stall for seconds or forever.
use commons_core::api::{bridge_request_ok, App};
use commons_core::bridge_auth::{self, Auth};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// One Update click at a time.
static INSTALLING: AtomicBool = AtomicBool::new(false);
/// This start replaced an older launcher script in the install folder (Linux).
static LAUNCHER_REFRESHED: AtomicBool = AtomicBool::new(false);

// Same policy as the desktop shell, widened only for what calls need: the LiveKit signalling socket, the frame-encryption worker, media streams.
// http:/https: on img-src, connect-src and media-src are for the owner's Jellyfin server, configured per device at an arbitrary
// address (often plain http on the LAN or Tailscale). A CSP cannot name a runtime-configured host, and Jellyfin media must be
// fetched and played by <video> directly (never proxied through the relay), so the widest part of the policy is limited to those
// three destinations. Scripts, workers and styles stay 'self'.
const CSP: &str = "default-src 'self'; img-src 'self' data: blob: http: https:; style-src 'self' 'unsafe-inline'; script-src 'self'; connect-src 'self' ws: wss: http: https:; worker-src 'self' blob:; media-src 'self' blob: mediastream: http: https:; base-uri 'none'; form-action 'none'; object-src 'none'";

/// How long an idle keep-alive connection is held before it is closed (its thread with it).
const KEEP_ALIVE_SECS: u64 = 60;

fn reply(s: &mut TcpStream, code: u16, ct: &str, body: &[u8], keep: bool) -> bool {
    let reason = match code { 200 => "OK", 403 => "Forbidden", 400 => "Bad Request", 404 => "Not Found", _ => "Error" };
    let conn = if keep { format!("keep-alive\r\nKeep-Alive: timeout={KEEP_ALIVE_SECS}") } else { "close".to_string() };
    let head = format!("HTTP/1.1 {code} {reason}\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: {conn}\r\nCache-Control: no-store\r\nContent-Security-Policy: {CSP}\r\n\r\n", body.len());
    // One write: on a kept-alive connection a header written on its own would sit behind Nagle's algorithm until the
    // body's ACK came back, which is tens of milliseconds for every answer.
    let mut msg = Vec::with_capacity(head.len() + body.len());
    msg.extend_from_slice(head.as_bytes()); msg.extend_from_slice(body);
    let ok = s.write_all(&msg).and_then(|_| s.flush()).is_ok();
    ok && keep
}

/// The install folder and the update key, when this is an installed browser version. A bridge run from a source tree
/// has no key or no `bridge` beside its ui/, so it never updates anything.
fn updatable(ui: &std::path::Path) -> Option<(PathBuf, &'static str)> {
    use commons_core::browser_update::{bridge_name, public_key};
    let install = ui.parent()?;
    if !install.join(bridge_name()).is_file() { return None; }
    Some((install.to_path_buf(), public_key()?))
}

/// Where the launcher keeps its runtime files (pid, log, the open ticket): MEGATRON_NOTE_DIR, which for a macOS bundle is the
/// data folder (Contents/Resources is sealed), otherwise the install folder.
fn state_dir(ui: &std::path::Path, port: u16) -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("MEGATRON_NOTE_DIR") { return Some(PathBuf::from(d)); }
    let install = ui.parent()?;
    // A launcher that does not set it (an older one) must not make us write into a sealed .app bundle.
    if install.ends_with("Contents/Resources") { return Some(data_dir(port)); }
    Some(install.to_path_buf())
}

fn data_dir(port: u16) -> PathBuf {
    PathBuf::from(std::env::var("COMMONS_DATA").unwrap_or_else(|_| format!("./dev-data-{port}")))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
/// Appends a line to the launcher's log (the data folder for a macOS bundle), and to stderr (bridge.log).
fn launcher_log(ui: &std::path::Path, port: u16, msg: &str) {
    eprintln!("{msg}");
    if let Some(dir) = state_dir(ui, port) {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("launcher.log")) {
            let _ = writeln!(f, "bridge: {msg}");
        }
    }
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

/// One connection. The page's calls (several a second) arrive on a few connections the browser keeps open, so each
/// request no longer costs a TCP handshake, a thread and a walk through /proc/net/tcp; the owner check happens once per
/// connection, which is all it needs (a socket never changes hands). An idle connection is closed after KEEP_ALIVE_SECS.
fn handle(mut s: TcpStream, app: Arc<App>, ui: Arc<PathBuf>, port: u16, auth: Auth) {
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(KEEP_ALIVE_SECS)));
    let _ = s.set_nodelay(true);
    let mut r = BufReader::new(s.try_clone().expect("clone"));
    let mut checked = false;
    while request(&mut s, &mut r, &app, &ui, port, &auth, &mut checked) {}
}

/// Reads and answers one request. Returns true when the connection may carry another.
fn request(s: &mut TcpStream, r: &mut BufReader<TcpStream>, app: &App, ui: &std::path::Path, port: u16, auth: &Auth, checked: &mut bool) -> bool {
    let mut line = String::new();
    if r.read_line(&mut line).unwrap_or(0) == 0 { return false; }   // closed, or idle past the timeout
    let mut parts = line.split_whitespace();
    let (method, url, version) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
    let (mut len, mut host, mut origin, mut presented, mut close) = (0usize, None::<String>, None::<String>, None::<String>, version != "HTTP/1.1");
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).unwrap_or(0) == 0 { return false; }
        if h == "\r\n" || h == "\n" { break; }
        let low = h.to_ascii_lowercase();
        if let Some(v) = low.strip_prefix("content-length:") { len = v.trim().parse().unwrap_or(0); }
        if let Some(v) = low.strip_prefix("host:") { host = Some(v.trim().to_string()); }
        if let Some(v) = low.strip_prefix("origin:") { origin = Some(v.trim().to_string()); }
        if let Some(v) = low.strip_prefix("connection:") { close = v.contains("close"); }
        if low.starts_with(bridge_auth::HEADER) && low[bridge_auth::HEADER.len()..].starts_with(':') { presented = Some(h[bridge_auth::HEADER.len() + 1..].trim().to_string()); }
    }
    let keep = !close;
    if !bridge_request_ok(host.as_deref(), origin.as_deref(), port) { return reply(s, 403, "text/plain", b"forbidden", false); }
    // Another user's program on this computer can reach the port too. Linux can tell whose socket is on the other end.
    if !*checked {
        if let (Ok(peer), Ok(local)) = (s.peer_addr(), s.local_addr()) {
            if !bridge_auth::peer_is_same_user(peer, local) { return reply(s, 403, "text/plain", b"forbidden", false); }
        }
        *checked = true;
    }
    // Only /rpc carries a body. Anything else with one is answered and the connection closed, so a body is never read as the next request.
    let keep = keep && (len == 0 || (method == "POST" && url == "/rpc"));
    // (/session reads its small body and closes the connection.)
    let (route, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
    if method == "GET" && route == "/hello" {
        // The launcher's challenge: only a bridge that knows the secret can answer with the right proof.
        let nonce = query.split('&').find_map(|kv| kv.strip_prefix("c=")).unwrap_or("");
        return reply(s, 200, "application/json", auth.hello(nonce).to_string().as_bytes(), keep);
    }
    if method == "GET" && route == "/mode" {
        let body = serde_json::json!({"secret": !auth.is_open(), "packaged": updatable(ui).is_some(), "launcher_updated": LAUNCHER_REFRESHED.load(Ordering::SeqCst)});
        return reply(s, 200, "application/json", body.to_string().as_bytes(), keep);
    }
    if method == "POST" && route == "/session" {
        // A tab in the everyday browser arrives with a one-time ticket (/#t=...) instead of the secret, so the secret is never in
        // that browser's history. The ticket is spent by this call; the answer carries the secret.
        let Some(secret) = auth.secret() else { return reply(s, 200, "application/json", br#"{"open":true}"#, false) };
        if len > 512 { return reply(s, 400, "text/plain", b"too large", false); }
        let mut body = vec![0u8; len];
        if r.read_exact(&mut body).is_err() { return reply(s, 400, "text/plain", b"bad body", false); }
        let ticket = String::from_utf8_lossy(&body).trim().to_string();
        let ok = state_dir(ui, port).is_some_and(|d| bridge_auth::take_ticket(&d, port, &ticket));
        if !ok { return reply(s, 403, "text/plain", b"bad ticket", false); }
        return reply(s, 200, "application/json", serde_json::json!({"secret": secret}).to_string().as_bytes(), false);
    }
    let protected = route == "/open-as" ||
        route == "/rpc" || route == "/update-note" || route == "/update-check" || route == "/update-install";
    if protected && !auth.allows(presented.as_deref()) { return reply(s, 403, "text/plain", b"bad secret", false); }
    if route == "/open-as" {
        // The launcher's per-device choice: show the page in a window of its own, or as a tab in the everyday browser.
        // Only an installed copy has a launcher to honour it. The value is written to the data folder (`open-as`).
        if updatable(ui).is_none() { return reply(s, 200, "application/json", br#"{"packaged":false}"#, keep); }
        let data = data_dir(port);
        if method == "POST" {
            let Some(v) = query.split('&').find_map(|kv| kv.strip_prefix("v=")).and_then(bridge_auth::OpenAs::parse) else { return reply(s, 400, "text/plain", b"bad value", keep) };
            if let Err(e) = bridge_auth::write_open_as(&data, v) { return reply(s, 200, "application/json", serde_json::json!({"packaged": true, "error": e.to_string()}).to_string().as_bytes(), keep); }
        }
        let env = std::env::var("MEGATRON_OPEN").ok();
        let body = serde_json::json!({"packaged": true, "open_as": bridge_auth::open_as(&data, None).word(), "forced": env.as_deref().and_then(bridge_auth::OpenAs::parse).map(|v| v.word())});
        return reply(s, 200, "application/json", body.to_string().as_bytes(), keep);
    }
    if method == "GET" && url == "/update-note" {
        // What the launcher's update check left for the page (packaging/linux-browser/megatron). Read once, then gone.
        let note = ui.parent().and_then(commons_core::browser_update::take_note);
        let body = note.map(|n| n.to_string()).unwrap_or_else(|| "{}".into());
        return reply(s, 200, "application/json", body.as_bytes(), keep);
    }
    if method == "GET" && url == "/update-check" {
        let body = match updatable(ui) {
            None => serde_json::json!({"packaged": false}),
            Some(_) => {
                let relay = std::env::var("COMMONS_RELAY").unwrap_or_default();
                match commons_core::browser_update::check(&relay, commons_core::browser_update::version()) {
                    Ok(v) => v,
                    Err(e) => serde_json::json!({"error": e}),
                }
            }
        };
        return reply(s, 200, "application/json", body.to_string().as_bytes(), keep);
    }
    if method == "POST" && url == "/update-install" {
        let Some((install, key)) = updatable(ui) else {
            return reply(s, 200, "application/json", br#"{"packaged":false}"#, keep);
        };
        let out = update_install(app, &install, key);
        let more = reply(s, 200, "application/json", out.to_string().as_bytes(), keep);
        if out["restarting"] == true {
            let _ = s.shutdown(std::net::Shutdown::Both);
            restart(&install, ui, port);
        }
        return more;
    }
    if method == "POST" && url == "/rpc" {
        if len > 40 << 20 { return reply(s, 400, "text/plain", b"too large", false); }
        let mut body = vec![0u8; len];
        if r.read_exact(&mut body).is_err() { return reply(s, 400, "text/plain", b"bad body", false); }
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        let m = v["method"].as_str().unwrap_or("").to_string();
        let out = match app.call(&m, v["args"].clone()) { Ok(x) => serde_json::json!({"ok": x}), Err(e) => serde_json::json!({"err": e}) };
        return reply(s, 200, "application/json", out.to_string().as_bytes(), keep);
    }
    let Some(file) = bridge_auth::ui_file(ui, &url) else { return reply(s, 400, "text/plain", b"bad path", keep) };
    let path = file.to_string_lossy().to_string();
    match std::fs::read(&file) {
        Ok(b) => {
            let ct = if path.ends_with(".css") { "text/css" } else if path.ends_with(".js") { "text/javascript" } else if path.ends_with(".svg") { "image/svg+xml" } else { "text/html; charset=utf-8" };
            reply(s, 200, ct, &b, keep)
        }
        Err(_) => reply(s, 404, "text/plain", b"not found", keep),
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
    let auth = Auth::from_env();
    // macOS: an update brings the new launcher program inside ui/ (browser_update::MAC_LAUNCHER_CARRIER); it replaces
    // Contents/MacOS/launcher here, before the port is bound, so a launcher that is waiting for this bridge does not
    // sign the bundle at the same time.
    #[cfg(target_os = "macos")]
    if let (Some(install), Some(state)) = (ui.parent(), state_dir(&ui, port)) {
        use commons_core::browser_update::{refresh_mac_launcher, resign_bundle, LauncherSwap};
        match refresh_mac_launcher(install, &state, &|| resign_bundle(install)) {
            LauncherSwap::Replaced => { LAUNCHER_REFRESHED.store(true, Ordering::SeqCst); launcher_log(&ui, port, "replaced the older launcher with the current one and signed the app again"); }
            LauncherSwap::Same => launcher_log(&ui, port, "the launcher is already current; signed the app again"),
            LauncherSwap::Failed(e) => launcher_log(&ui, port, &format!("could not replace the launcher: {e}")),
            LauncherSwap::Skipped | LauncherSwap::Nothing => {}
        }
    }
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
    // An installed copy whose launcher script is older than this bridge gets the current one (Linux; the old file is kept as
    // megatron.old). A launcher that speaks the secret protocol is what turns the protection on at the next start.
    if updatable(&ui).is_some() && ui.parent().is_some_and(bridge_auth::refresh_linux_launcher) {
        LAUNCHER_REFRESHED.store(true, Ordering::SeqCst);
        eprintln!("replaced the older launcher script with the current one");
    }
    eprintln!("dev bridge on http://127.0.0.1:{port}  (ui: {}; {})", ui.display(),
        if auth.is_open() { "open mode: no secret was given" } else { "secret required" });
    for conn in listener.incoming().flatten() {
        let (app, ui, auth) = (app.clone(), ui.clone(), auth.clone());
        std::thread::spawn(move || handle(conn, app, ui, port, auth));
    }
}
