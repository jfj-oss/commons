#![cfg_attr(windows, windows_subsystem = "windows")]
//! The launcher for the Windows and macOS "browser version" of Megatron (packaging/windows-browser,
//! packaging/macos-browser; docs/UPDATES.md).
//!
//! It starts the core bridge (`bridge` / `bridge.exe`) in the background if it is not already running, then shows the
//! UI in Brave, Chrome or Edge as an app window with its own profile folder. On Windows Edge is always present; on
//! macOS Safari is the fallback, as a normal tab.
//!
//! Before starting the bridge it gives `bridge --update <relay> <install-dir>` about five seconds to install a signed
//! update, exactly as the Linux shell launcher does. On Windows that child process is the running `bridge.exe` itself,
//! so the update renames the running image aside and moves the new one into place (Windows cannot overwrite a running
//! executable, but it can rename it). If the new bridge does not come up, the previous bridge and `ui/` are put back.
//!
//!   launcher            open Megatron (starting the bridge if needed)
//!   launcher --stop     stop the background bridge
//!
//! Nothing is printed: on Windows the program has no console at all, and on macOS it is started from the bundle. The
//! log is `launcher.log` in the install folder (for a macOS bundle, the data folder: `Contents/Resources` is signed
//! and must not change), and the bridge writes `bridge.log` beside it.
//!
//! Who may talk to the bridge (docs/THREAT-MODEL.md, "The browser edition"): every start of the bridge gets a new random
//! secret (MEGATRON_SECRET), kept in `.bridge-secret-<port>` in the data folder (mode 600), and the window opens at
//! `http://127.0.0.1:<port>/#s=<secret>`. Before opening, the launcher asks whatever answers on the port to prove it knows the
//! secret (`/hello`); a program that cannot is either an older bridge of ours (restarted with a secret) or something else
//! (refused with a message). The window uses its own profile folder, `browser-profile` in the data folder.
//!
//! Window or tab: by default the UI is an app window with its own profile. `open-as` in the data folder (`tab` or `window`, set in
//! mega's Settings) or MEGATRON_OPEN=tab|window opens it as a tab in the person's own browser instead; the address then holds a
//! one-time ticket, not the secret, because that browser keeps its history (docs/THREAT-MODEL.md).
//!
//! Environment: MEGATRON_PORT (default 7420), MEGATRON_DATA (the account folder), MEGATRON_NO_UPDATE=1 and
//! MEGATRON_NO_BROWSER=1 (tests and headless use), MEGATRON_BROWSER=<path> (a browser to use instead of finding one).
//! The launcher sets MEGATRON_NOTE_DIR for the bridge: it is the folder the update note is read from (the data folder
//! inside a macOS bundle, whose Contents/Resources must not change after signing).

use commons_core::browser_update::{bridge_name, restore_previous};
#[cfg(target_os = "macos")]
use commons_core::browser_update::resign_bundle;
use commons_core::bridge_auth::{open_as, OpenAs};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const DEFAULT_PORT: u16 = 7420;
const DEFAULT_RELAY: &str = "http://127.0.0.1:8787";

fn install_dir() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    // Inside Megatron.app the launcher is Contents/MacOS/launcher (the only file codesign may treat as code there),
    // and the bridge, ui/ and relay.txt sit next to it in Contents/Resources (scripts/build-mac-browser.sh).
    #[cfg(target_os = "macos")]
    {
        if exe_dir.file_name().and_then(|n| n.to_str()) == Some("MacOS") {
            if let Some(contents) = exe_dir.parent().filter(|p| p.file_name().and_then(|n| n.to_str()) == Some("Contents")) {
                let resources = contents.join("Resources");
                if resources.is_dir() {
                    return resources;
                }
            }
        }
    }
    exe_dir
}

/// True when `install` is `Megatron.app/Contents/Resources`: a signed macOS bundle. Its Resources are sealed by
/// codesign, so runtime files (pid, logs, the update note) must not live there; every write would break the seal.
#[cfg(target_os = "macos")]
fn in_bundle(install: &Path) -> bool {
    let Some(contents) = install.parent() else { return false };
    let Some(app) = contents.parent() else { return false };
    install.file_name().and_then(|n| n.to_str()) == Some("Resources")
        && contents.file_name().and_then(|n| n.to_str()) == Some("Contents")
        && app.extension().and_then(|e| e.to_str()) == Some("app")
}

#[cfg(not(target_os = "macos"))]
fn in_bundle(_install: &Path) -> bool {
    false
}

/// Where the launcher and bridge keep the pid, their logs and the update note: the install folder normally, the data
/// folder for a macOS app bundle (see in_bundle).
fn state_dir(install: &Path) -> PathBuf {
    if in_bundle(install) {
        data_dir(install)
    } else {
        install.to_path_buf()
    }
}

fn log(install: &Path, msg: &str) {
    let dir = state_dir(install);
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("launcher.log")) {
        let _ = writeln!(f, "{msg}");
    }
}

fn data_dir(install: &Path) -> PathBuf {
    if let Some(d) = std::env::var_os("MEGATRON_DATA") {
        return PathBuf::from(d);
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(d) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(d).join("Megatron-browser").join("data");
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(h) = std::env::var_os("HOME") {
            return PathBuf::from(h).join("Library").join("Application Support").join("Megatron-browser");
        }
    }
    install.join("data")
}

fn port() -> u16 {
    std::env::var("MEGATRON_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT)
}

fn relay(install: &Path) -> String {
    std::fs::read_to_string(install.join("relay.txt")).map(|s| s.trim().to_string()).unwrap_or_else(|_| DEFAULT_RELAY.to_string())
}

/// What answers on the port, judged by the secret challenge.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Port {
    /// Nothing is listening.
    Free,
    /// A bridge that knows our secret.
    Ours,
    /// Something answers but cannot prove it: an older bridge of ours, a bridge started without a secret, or another program.
    Other,
}

/// `GET /hello?c=<nonce>` with a short timeout: the JSON body, if there is an HTTP answer at all.
fn hello(port: u16, nonce: &str) -> Option<serde_json::Value> {
    use std::io::Read;
    use std::net::{SocketAddr, TcpStream};
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(400)).ok()?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    write!(s, "GET /hello?c={nonce} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").ok()?;
    let mut buf = Vec::new();
    let _ = s.take(64 * 1024).read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf).to_string();
    let body = text.split_once("\r\n\r\n")?.1;
    serde_json::from_str(body.trim()).ok()
}

fn probe(port: u16, secret: Option<&str>) -> Port {
    if !up(port) {
        return Port::Free;
    }
    let (Some(secret), nonce) = (secret, commons_core::bridge_auth::new_secret()[..24].to_string()) else { return Port::Other };
    match hello(port, &nonce) {
        Some(v) if v["proof"].as_str().is_some_and(|p| commons_core::bridge_auth::ct_eq(p, &commons_core::bridge_auth::proof(secret, &nonce))) => Port::Ours,
        _ => Port::Other,
    }
}

fn secret_path(data: &Path, port: u16) -> PathBuf {
    data.join(format!(".bridge-secret-{port}"))
}

fn up(port: u16) -> bool {
    use std::net::{SocketAddr, TcpStream};
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    TcpStream::connect_timeout(&addr, Duration::from_millis(400)).is_ok()
}

#[cfg(windows)]
mod win {
    pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    pub const DETACHED_PROCESS: u32 = 0x0000_0008;
    pub const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    pub const COMMON: u32 = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP;
}

/// Keeps two launchers started at the same moment from running `bridge --update` at once (the Linux launcher uses
/// `flock` on the same file name). A lock left behind by an update that was killed is ignored after a minute.
struct UpdateLock(PathBuf);

impl UpdateLock {
    fn take(install: &Path) -> Option<UpdateLock> {
        let path = install.join(".update.lock");
        for attempt in 0..2 {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    let _ = writeln!(f, "{}", std::process::id());
                    return Some(UpdateLock(path));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|d| d.as_secs() > 60);
                    if attempt == 0 && stale {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    return None;
                }
                Err(_) => return None,
            }
        }
        None
    }
}

impl Drop for UpdateLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Runs `bridge --update`. Returns true when the update left a previous copy behind, i.e. something was installed.
fn run_update(install: &Path, relay: &str) -> bool {
    if std::env::var_os("MEGATRON_NO_UPDATE").is_some() || relay.is_empty() {
        return false;
    }
    let bridge = install.join(bridge_name());
    if !bridge.is_file() {
        return false;
    }
    let mut cmd = Command::new(&bridge);
    cmd.arg("--update").arg(relay).arg(install)
        .env("MEGATRON_NOTE_DIR", state_dir(install))
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(win::COMMON);
    }
    log(install, &format!("update: checking {relay}"));
    let Ok(mut child) = cmd.spawn() else {
        log(install, "update: could not start the check");
        return false;
    };
    // The Linux launcher caps this at five seconds. The bridge stops before downloading or swapping if it runs out of
    // time, so a slow or offline relay only holds the window a moment.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                log(install, &format!("update: finished ({status})"));
                break;
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    log(install, "update: took too long; starting without it");
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break,
        }
    }
    install.join(".update-previous").is_dir()
}

fn start_bridge(install: &Path, ui: &Path, data: &Path, relay: &str, port: u16, secret: &str) -> Option<std::process::Child> {
    let bridge = install.join(bridge_name());
    let state = state_dir(install);
    let _ = std::fs::create_dir_all(&state);
    let log_file = std::fs::OpenOptions::new().create(true).write(true).truncate(true).open(state.join("bridge.log")).ok();
    let mut cmd = Command::new(&bridge);
    cmd.arg(port.to_string())
        .arg(ui)
        .env("COMMONS_DATA", data)
        .env("COMMONS_RELAY", relay)
        .env("MEGATRON_NOTE_DIR", &state)
        .env("MEGATRON_SECRET", secret)
        .stdin(Stdio::null());
    match log_file {
        Some(f) => {
            let err = f.try_clone().ok();
            cmd.stdout(Stdio::from(f));
            match err {
                Some(e) => cmd.stderr(Stdio::from(e)),
                None => cmd.stderr(Stdio::null()),
            };
        }
        None => {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(win::DETACHED_PROCESS | win::COMMON);
    }
    match cmd.spawn() {
        Ok(child) => Some(child),
        Err(e) => {
            log(install, &format!("could not start {}: {e}", bridge.display()));
            None
        }
    }
}

fn stop_pid(pid: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(win::COMMON)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("kill").arg("-TERM").arg(pid.to_string()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
}

fn stop(install: &Path) {
    let pidfile = state_dir(install).join(".pid");
    if let Ok(text) = std::fs::read_to_string(&pidfile) {
        if let Ok(pid) = text.trim().parse::<u32>() {
            stop_pid(pid);
            log(install, &format!("stopped bridge {pid}"));
        }
    }
    let _ = std::fs::remove_file(&pidfile);
    let _ = std::fs::remove_file(secret_path(&data_dir(install), port()));
}

#[cfg(windows)]
fn alert(msg: &str) {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = std::ffi::OsStr::new(msg).encode_wide().chain(std::iter::once(0)).collect();
    let cap: Vec<u16> = "mega".encode_utf16().chain(std::iter::once(0)).collect();
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(hwnd: *mut core::ffi::c_void, text: *const u16, caption: *const u16, utype: u32) -> i32;
    }
    unsafe {
        MessageBoxW(std::ptr::null_mut(), wide.as_ptr(), cap.as_ptr(), 0x10);
    }
}

#[cfg(target_os = "macos")]
fn alert(msg: &str) {
    let escaped = msg.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        "display dialog \"{escaped}\" buttons {{\"OK\"}} default button \"OK\" with title \"mega\" with icon caution"
    );
    let _ = Command::new("osascript").arg("-e").arg(script).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

#[cfg(all(unix, not(target_os = "macos")))]
fn alert(msg: &str) {
    let _ = Command::new("notify-send").args(["mega", msg]).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

#[cfg(windows)]
fn browser_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    let pf = std::env::var_os("ProgramFiles").map(PathBuf::from);
    let pfx86 = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from);
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    // Brave, then Chrome, then Edge (Edge is on every Windows 10 and 11).
    if let Some(p) = local.as_ref() {
        v.push(p.join(r"BraveSoftware\Brave-Browser\Application\brave.exe"));
        v.push(p.join(r"Google\Chrome\Application\chrome.exe"));
    }
    if let Some(p) = pf.as_ref() {
        v.push(p.join(r"BraveSoftware\Brave-Browser\Application\brave.exe"));
        v.push(p.join(r"Google\Chrome\Application\chrome.exe"));
        v.push(p.join(r"Microsoft\Edge\Application\msedge.exe"));
    }
    if let Some(p) = pfx86.as_ref() {
        v.push(p.join(r"BraveSoftware\Brave-Browser\Application\brave.exe"));
        v.push(p.join(r"Google\Chrome\Application\chrome.exe"));
        v.push(p.join(r"Microsoft\Edge\Application\msedge.exe"));
    }
    v
}

#[cfg(target_os = "macos")]
fn browser_paths() -> Vec<PathBuf> {
    let mut apps = vec![PathBuf::from("/Applications")];
    if let Some(home) = std::env::var_os("HOME") {
        apps.push(PathBuf::from(home).join("Applications"));
    }
    let mut v = Vec::new();
    for app in apps {
        v.push(app.join("Brave Browser.app/Contents/MacOS/Brave Browser"));
        v.push(app.join("Google Chrome.app/Contents/MacOS/Google Chrome"));
        v.push(app.join("Microsoft Edge.app/Contents/MacOS/Microsoft Edge"));
    }
    v
}

#[cfg(not(any(windows, target_os = "macos")))]
fn browser_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    for name in ["brave", "brave-browser", "google-chrome", "google-chrome-stable", "chromium", "microsoft-edge"] {
        if let Some(paths) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&paths) {
                let p = dir.join(name);
                if p.is_file() {
                    v.push(p);
                }
            }
        }
    }
    v
}

/// Opens the UI as a tab in the person's own browser (profile and all), with no window of ours: on macOS `open -a` Brave if it
/// is installed, otherwise the default browser; on Windows and Linux the system's default browser (whatever the person chose,
/// Firefox included), and only if that cannot be started the first Chromium-family browser found, handed only the address.
fn open_tab(install: &Path, url: &str) -> bool {
    #[cfg(not(target_os = "macos"))]
    if std::env::var_os("MEGATRON_BROWSER").is_none() && default_browser(install, url, false) {
        return true;
    }
    #[cfg(target_os = "macos")]
    {
        let mut apps = vec![PathBuf::from("/Applications")];
        if let Some(h) = std::env::var_os("HOME") {
            apps.push(PathBuf::from(h).join("Applications"));
        }
        let brave = apps.iter().any(|d| d.join("Brave Browser.app").is_dir());
        if std::env::var_os("MEGATRON_BROWSER").is_none() {
            let mut c = Command::new("open");
            if brave { c.arg("-a").arg("Brave Browser"); }
            let ok = c.arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
            log(install, &format!("opened a tab in {}", if brave { "Brave" } else { "the default browser" }));
            return ok;
        }
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(b) = std::env::var_os("MEGATRON_BROWSER") {
        candidates.push(PathBuf::from(b));
    }
    #[cfg(not(target_os = "macos"))]
    candidates.extend(browser_paths());
    for exe in candidates.into_iter().filter(|p| p.is_file()) {
        let mut cmd = Command::new(&exe);
        cmd.arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(win::COMMON);
        }
        match cmd.spawn() {
            Ok(child) => {
                log(install, &format!("opened a tab with {} (pid {})", exe.display(), child.id()));
                return true;
            }
            Err(e) => log(install, &format!("could not open {}: {e}", exe.display())),
        }
    }
    #[cfg(target_os = "macos")]
    { default_browser(install, url, false) }
    #[cfg(not(target_os = "macos"))]
    { false }   // the default browser was tried first and could not be started
}

/// The system's own handler for an address (Windows, Linux), or Safari when asked (macOS, no Chromium browser found).
fn default_browser(install: &Path, url: &str, safari: bool) -> bool {
    #[cfg(target_os = "macos")]
    let mut fallback = {
        let mut c = Command::new("open");
        if safari { c.arg("-a").arg("Safari"); }
        c.arg(url);
        c
    };
    #[cfg(windows)]
    let mut fallback = {
        use std::os::windows::process::CommandExt;
        let _ = safari;
        let mut c = Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler").arg(url).creation_flags(win::COMMON);
        c
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    let mut fallback = {
        let _ = safari;
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    let ok = fallback.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().is_ok();
    log(install, if ok { "opened the fallback browser" } else { "no browser could be opened" });
    ok
}

/// Opens the UI as an app window. Safari (macOS) or the default browser is the last resort, as a normal tab.
fn open_browser(install: &Path, data: &Path, url: &str, how: OpenAs) -> bool {
    if how == OpenAs::Tab {
        return open_tab(install, url);
    }
    let profile = data.join("browser-profile");
    let _ = std::fs::create_dir_all(&profile);
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(b) = std::env::var_os("MEGATRON_BROWSER") {
        candidates.push(PathBuf::from(b));
    }
    candidates.extend(browser_paths());
    for exe in candidates.into_iter().filter(|p| p.is_file()) {
        let mut cmd = Command::new(&exe);
        cmd.arg(format!("--app={url}"))
            .arg("--user-data-dir")
            .arg(&profile)
            .args(["--no-first-run", "--no-default-browser-check"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(win::COMMON);
        }
        match cmd.spawn() {
            Ok(child) => {
                log(install, &format!("opened {} (pid {})", exe.display(), child.id()));
                return true;
            }
            Err(e) => log(install, &format!("could not open {}: {e}", exe.display())),
        }
    }
    // No Chromium-family browser found: Safari on macOS, otherwise whatever the system uses.
    default_browser(install, url, true)
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Launch {
    /// The new bridge proved it knows the secret.
    Secret,
    /// A bridge is serving but never proved it: an older build that ignores MEGATRON_SECRET.
    OlderBridge,
    Failed,
}

/// Starts the bridge with a new secret and waits until it proves it has it. A bridge that stays alive and serves but does not
/// prove the secret is an older build; one that dies (for instance because a program took the port first) is a failure.
fn launch(install: &Path, ui: &Path, data: &Path, relay: &str, port: u16) -> (Launch, String) {
    let secret = commons_core::bridge_auth::new_secret();
    if let Err(e) = commons_core::bridge_auth::write_secret_file(&secret_path(data, port), &secret) {
        log(install, &format!("could not keep the secret: {e}"));
        return (Launch::Failed, secret);
    }
    let Some(mut child) = start_bridge(install, ui, data, relay, port, &secret) else { return (Launch::Failed, secret) };
    let _ = std::fs::write(state_dir(install).join(".pid"), child.id().to_string());
    // A new bridge waits up to five seconds for a port that the previous one is still leaving, so wait a little longer.
    let end = Instant::now() + Duration::from_secs(7);
    while Instant::now() < end {
        if probe(port, Some(&secret)) == Port::Ours {
            return (Launch::Secret, secret);
        }
        if child.try_wait().ok().flatten().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if child.try_wait().ok().flatten().is_none() && up(port) {
        return (Launch::OlderBridge, secret);
    }
    stop_pid(child.id());
    let _ = std::fs::remove_file(state_dir(install).join(".pid"));
    (Launch::Failed, secret)
}

fn main() {
    let install = install_dir();
    let _ = std::fs::create_dir_all(&install);
    let port = port();
    if std::env::args().skip(1).any(|a| a == "--stop") {
        stop(&install);
        return;
    }
    let data = data_dir(&install);
    if let Err(e) = std::fs::create_dir_all(&data) {
        alert(&format!("mega could not make its data folder ({}): {e}", data.display()));
        return;
    }
    let relay = relay(&install);
    let ui = install.join("ui");
    log(&install, &format!("launch: install {}, data {}, port {port}", install.display(), data.display()));

    let secret_file = secret_path(&data, port);
    let mut state = probe(port, commons_core::bridge_auth::read_secret_file(&secret_file).as_deref());
    let mut secret = commons_core::bridge_auth::read_secret_file(&secret_file);
    if state == Port::Other {
        // Something holds the port and cannot prove our secret. Our own older bridge (its pid is in .pid) is restarted with a
        // secret; anything else is not ours to touch and the window is not opened onto it.
        let pidfile = state_dir(&install).join(".pid");
        let pid = std::fs::read_to_string(&pidfile).ok().and_then(|t| t.trim().parse::<u32>().ok());
        let Some(pid) = pid else {
            log(&install, &format!("port {port} is held by something that is not this bridge; refusing"));
            alert(&format!("Port {port} is used by another program, not by mega, so mega will not open there. Quit that program, or start mega on another port (set MEGATRON_PORT)."));
            return;
        };
        log(&install, &format!("port {port} answers without the secret; restarting our bridge {pid}"));
        stop_pid(pid);
        let _ = std::fs::remove_file(&pidfile);
        let end = Instant::now() + Duration::from_secs(5);
        while up(port) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(100));
        }
        if up(port) {
            alert(&format!("Port {port} is still used by another program, so mega will not open there. Quit that program, or set MEGATRON_PORT."));
            return;
        }
        state = Port::Free;
    }
    let mut legacy = false;
    if state == Port::Free {
        let lock = UpdateLock::take(&install);
        let updated = if lock.is_some() {
            run_update(&install, &relay)
        } else {
            log(&install, "update: another copy is checking; skipped");
            false
        };
        drop(lock);
        #[cfg(not(target_os = "macos"))]
        let _ = updated;
        let mut launched = launch(&install, &ui, &data, &relay, port);
        if launched.0 == Launch::Failed {
            if restore_previous(&install) {
                log(&install, "the new version did not start; restored the previous one");
                commons_core::browser_update::cleanup_previous(&install);
                #[cfg(target_os = "macos")]
                resign_bundle(&install);
                launched = launch(&install, &ui, &data, &relay, port);
            }
        } else {
            // A macOS bundle must be re-signed after the update changed Contents/Resources. The new bridge has removed
            // .update-previous by now (it does that right after binding its port); do it here too, so the seal covers
            // exactly what stays on disk. Signing earlier would seal the backup, which the bridge then deletes.
            #[cfg(target_os = "macos")]
            if updated {
                commons_core::browser_update::cleanup_previous(&install);
                resign_bundle(&install);
            }
        }
        if launched.0 == Launch::Failed || !up(port) {
            log(&install, "the bridge did not answer");
            let log_path = state_dir(&install).join("bridge.log");
            alert(&format!("mega could not start. See {} for details.", log_path.display()));
            return;
        }
        legacy = launched.0 == Launch::OlderBridge;
        secret = Some(launched.1);
        if legacy {
            log(&install, "the bridge is an older build that ignores the secret; opening without one");
            alert("This copy of mega's background part is too old to protect the window with a key. It works, but other programs on this computer could reach it. Install the newest mega to fix that.");
        }
    }
    let how = open_as(&data, std::env::var("MEGATRON_OPEN").ok().as_deref());
    let url = match (&secret, legacy) {
        (Some(_), false) => {
            // The address (and with --app= the browser's command line, which other users on the computer can read) carries a one-time
            // ticket, never the secret: the page trades it for the secret at /session, and it is spent then (or after two minutes).
            // A tab also lands in the everyday browser, which keeps every address it visits.
            let ticket = commons_core::bridge_auth::new_secret();
            match commons_core::bridge_auth::write_secret_file(&commons_core::bridge_auth::ticket_path(&state_dir(&install), port), &ticket) {
                Ok(()) => format!("http://127.0.0.1:{port}/#t={ticket}"),
                Err(e) => {
                    log(&install, &format!("could not write the open ticket ({e}); not opening, so that the secret is never put in an address"));
                    alert(&format!("mega could not write its start-up ticket in {} ({e}), so it did not open.", state_dir(&install).display()));
                    return;
                }
            }
        }
        _ => format!("http://127.0.0.1:{port}/"),
    };
    if std::env::var_os("MEGATRON_NO_BROWSER").is_none() {
        open_browser(&install, &data, &url, how);
    } else if how == OpenAs::Tab {
        log(&install, &format!("tab mode; address would be {}", url.split('#').next().unwrap_or("")));
    }
    log(&install, "done");
}
