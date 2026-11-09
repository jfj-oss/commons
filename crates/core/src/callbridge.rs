//! "Open this call in my browser": for desktops whose web view cannot do calls (WebKitGTK builds without WebRTC).
//! A tiny loopback-only HTTP server serves the stand-alone call page and answers exactly four rpc methods, `call_state`,
//! `call_token`, and for game keys `game_mute_wait` and `game_mute_clear`, for ONE room (plus the five `pcaudio_*` sound-route methods), to whoever presents the one-time secret that is only in the opened tab's URL fragment.
//! Host header must be `127.0.0.1:<port>` (DNS-rebinding defence); no CORS headers are sent; the secret is checked in constant
//! time; every opening creates a new secret and revokes the old one. The media keys then live in that browser tab (see THREAT-MODEL.md).
use crate::api::App;
use rand::RngCore;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const FILES: &[(&str, &str, &[u8])] = &[
    ("/callpage.html", "text/html; charset=utf-8", include_bytes!("../../../app/ui/callpage.html")),
    ("/callpage.js", "text/javascript", include_bytes!("../../../app/ui/callpage.js")),
    ("/mic-gate.js", "text/javascript", include_bytes!("../../../app/ui/mic-gate.js")),
    ("/call.js", "text/javascript", include_bytes!("../../../app/ui/call.js")),
    ("/call-ui.js", "text/javascript", include_bytes!("../../../app/ui/call-ui.js")),
    ("/call-pcaudio.js", "text/javascript", include_bytes!("../../../app/ui/call-pcaudio.js")),
    ("/call.css", "text/css", include_bytes!("../../../app/ui/call.css")),
    ("/vendor/livekit-client.umd.js", "text/javascript", include_bytes!("../../../app/ui/vendor/livekit-client.umd.js")),
    ("/vendor/livekit-client.e2ee.worker.js", "text/javascript", include_bytes!("../../../app/ui/vendor/livekit-client.e2ee.worker.js")),
];

/// A secret stops working after this long (a call that runs longer is re-opened from the app).
const SECRET_TTL: std::time::Duration = std::time::Duration::from_secs(12 * 3600);
/// The page loads only its own files and may talk to the relay's call server; nothing inline, nothing framed.
/// http:/https: on img-src, connect-src and media-src exist for Jellyfin: its address is configured per device at runtime
/// (often plain http on the LAN or Tailscale), so it cannot be named here, and its media has to be fetched and played by
/// <video> directly (never proxied through the relay). Scripts, workers, styles and frames stay locked down.
const CSP: &str = "default-src 'self'; img-src 'self' data: blob: http: https:; style-src 'self' 'unsafe-inline'; script-src 'self'; connect-src 'self' ws: wss: https: http:; worker-src 'self' blob:; media-src 'self' blob: mediastream: http: https:; base-uri 'none'; form-action 'none'; object-src 'none'; frame-ancestors 'none'";

struct Grant { secret: String, gid: String, issued: std::time::Instant }
impl Default for Grant { fn default() -> Self { Grant { secret: String::new(), gid: String::new(), issued: std::time::Instant::now() } } }
pub struct CallBridge { pub port: u16, grant: Arc<Mutex<Grant>>, stopping: Arc<AtomicBool>, thread: Mutex<Option<std::thread::JoinHandle<()>>> }

fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && !a.is_empty() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}
fn reply(s: &mut TcpStream, code: u16, ct: &str, body: &[u8]) {
    let reason = match code { 200 => "OK", 400 => "Bad Request", 403 => "Forbidden", 404 => "Not Found", _ => "Error" };
    let head = format!("HTTP/1.1 {code} {reason}\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: {CSP}\r\n\r\n", body.len());
    let _ = s.write_all(head.as_bytes()).and_then(|_| s.write_all(body));
}

impl CallBridge {
    pub fn start(app: Arc<App>) -> std::io::Result<CallBridge> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let grant = Arc::new(Mutex::new(Grant::default()));
        let g2 = grant.clone();
        let stopping = Arc::new(AtomicBool::new(false));
        let st2 = stopping.clone();
        let thread = std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                if st2.load(Ordering::SeqCst) { break; }
                let (app, g) = (app.clone(), g2.clone());
                std::thread::spawn(move || handle(conn, app, g, port));
            }
        });
        Ok(CallBridge { port, grant, stopping, thread: Mutex::new(Some(thread)) })
    }
    /// Stop accepting and end the listener thread (which holds the app alive). Safe to call twice; the secret stops working too.
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        *self.grant.lock().unwrap() = Grant::default();
        let Some(t) = self.thread.lock().unwrap().take() else { return };
        // accept() is blocking: one connection to ourselves wakes it so it can see the flag.
        let _ = TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], self.port)), std::time::Duration::from_secs(2));
        let _ = t.join();
    }
    #[doc(hidden)]
    pub fn is_stopped(&self) -> bool { self.thread.lock().unwrap().is_none() }
    /// Authorise one room; returns the URL to open in the browser.
    pub fn grant(&self, gid: &str, name: &str) -> String {
        let mut b = [0u8; 32]; rand::rngs::OsRng.fill_bytes(&mut b);
        let secret = hex::encode(b);
        *self.grant.lock().unwrap() = Grant { secret: secret.clone(), gid: gid.into(), issued: std::time::Instant::now() };
        let enc = |s: &str| s.bytes().map(|c| if c.is_ascii_alphanumeric() { (c as char).to_string() } else { format!("%{c:02X}") }).collect::<String>();
        format!("http://127.0.0.1:{}/callpage.html#s={}&g={}&n={}", self.port, secret, gid, enc(name))
    }
}
impl Drop for CallBridge { fn drop(&mut self) { self.stop(); } }

fn handle(mut s: TcpStream, app: Arc<App>, grant: Arc<Mutex<Grant>>, port: u16) {
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(10)));
    let mut r = BufReader::new(match s.try_clone() { Ok(c) => c, Err(_) => return });
    let mut line = String::new();
    if r.read_line(&mut line).unwrap_or(0) == 0 { return; }
    let mut parts = line.split_whitespace();
    let (method, url) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
    let (mut len, mut host, mut secret) = (0usize, String::new(), String::new());
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" || h == "\n" { break; }
        let l = h.to_ascii_lowercase();
        if let Some(v) = l.strip_prefix("content-length:") { len = v.trim().parse().unwrap_or(0); }
        if let Some(v) = l.strip_prefix("host:") { host = v.trim().to_string(); }
        if let Some(v) = l.strip_prefix("x-call-secret:") { secret = v.trim().to_string(); }
    }
    if host != format!("127.0.0.1:{port}") { return reply(&mut s, 400, "text/plain", b"bad host"); }
    if method == "POST" && url == "/rpc" {
        if len > 4096 { return reply(&mut s, 400, "text/plain", b"too big"); }
        let mut body = vec![0u8; len];
        if r.read_exact(&mut body).is_err() { return reply(&mut s, 400, "text/plain", b"bad body"); }
        let want = { let g = grant.lock().unwrap(); if g.issued.elapsed() > SECRET_TTL { (String::new(), String::new()) } else { (g.secret.clone(), g.gid.clone()) } };
        if !ct_eq(&secret, &want.0) { return reply(&mut s, 403, "text/plain", b"bad secret"); }
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        let m = v["method"].as_str().unwrap_or("");
        let gid = v["args"]["gid"].as_str().unwrap_or("");
        // The room's own methods must name this room. The sound-route methods (pcaudio_*, Linux only) have no room: they act on
        // this computer's sound, and are guarded by the same one-time secret as everything else here.
        let room_ok = matches!(m, "call_state" | "call_token" | "game_mute_wait" | "game_mute_clear") && gid == want.1;
        let pc_ok = matches!(m, "pcaudio_caps" | "pcaudio_streams" | "pcaudio_start" | "pcaudio_status" | "pcaudio_stop");
        if !room_ok && !pc_ok { return reply(&mut s, 403, "text/plain", b"method or room not allowed"); }
        let out = match app.call(m, v["args"].clone()) { Ok(x) => serde_json::json!({"ok": x}), Err(e) => serde_json::json!({"err": e}) };
        return reply(&mut s, 200, "application/json", out.to_string().as_bytes());
    }
    let path = url.split(['?', '#']).next().unwrap_or("");
    match (method.as_str(), FILES.iter().find(|f| f.0 == path)) {
        ("GET", Some(f)) => reply(&mut s, 200, f.1, f.2),
        _ => reply(&mut s, 404, "text/plain", b"not found"),
    }
}

/// Open a URL in the system's default browser (never in the web view).
pub fn open_in_browser(url: &str) -> bool {
    #[cfg(target_os = "linux")] let r = {
        // Inside an AppImage these point at its own libraries, and a browser started with them can fail to start.
        let mut c = std::process::Command::new("xdg-open");
        if let Ok(dir) = std::env::var("APPDIR") {
            for v in ["LD_LIBRARY_PATH", "GTK_PATH", "GTK_DATA_PREFIX", "GTK_EXE_PREFIX", "GTK_IM_MODULE_FILE", "GTK_THEME", "GDK_PIXBUF_MODULE_FILE", "GIO_MODULE_DIR", "GSETTINGS_SCHEMA_DIR", "GI_TYPELIB_PATH", "APPDIR", "APPIMAGE", "ARGV0", "OWD"] { c.env_remove(v); }
            if let Ok(d) = std::env::var("XDG_DATA_DIRS") { c.env("XDG_DATA_DIRS", d.split(':').filter(|p| !p.starts_with(&dir)).collect::<Vec<_>>().join(":")); }
        }
        c.arg(url).spawn()
    };
    #[cfg(target_os = "macos")] let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")] let r = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn();
    // iOS has no way to start another program; the app opens the link itself (tauri-plugin-opener) and never calls this.
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    let r: std::io::Result<std::process::Child> = Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "opening a browser is not available here"));
    r.is_ok()
}
