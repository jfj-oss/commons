//! Development bridge: serves the UI over http://127.0.0.1 and exposes the same `rpc` the Tauri shell has.
//! Used for headless UI checks and for trying the app in two browser tabs without Tauri. Loopback only, NO authentication.
//!   COMMONS_DATA=/tmp/alice COMMONS_RELAY=http://127.0.0.1:8787 dev_bridge 7001 ../../app/ui
//! A small thread-per-connection HTTP/1.1 server (every response closes its connection). It replaced tiny_http, whose
//! fixed worker pool made a browser's parallel script downloads (the 600 KB call library) stall for seconds or forever.
use commons_core::api::App;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;

fn reply(s: &mut TcpStream, code: u16, ct: &str, body: &[u8]) {
    let reason = match code { 200 => "OK", 400 => "Bad Request", 404 => "Not Found", _ => "Error" };
    let head = format!("HTTP/1.1 {code} {reason}\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n", body.len());
    let _ = s.write_all(head.as_bytes()).and_then(|_| s.write_all(body));
    let _ = s.flush();
}

fn handle(mut s: TcpStream, app: Arc<App>, ui: Arc<PathBuf>) {
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(30)));
    let mut r = BufReader::new(s.try_clone().expect("clone"));
    let mut line = String::new();
    if r.read_line(&mut line).unwrap_or(0) == 0 { return; }
    let mut parts = line.split_whitespace();
    let (method, url) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
    let mut len = 0usize;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" || h == "\n" { break; }
        if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") { len = v.trim().parse().unwrap_or(0); }
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
    let port: u16 = args.next().and_then(|p| p.parse().ok()).unwrap_or(7001);
    let ui = Arc::new(PathBuf::from(args.next().unwrap_or_else(|| "app/ui".into())));
    let data = std::env::var("COMMONS_DATA").unwrap_or_else(|_| format!("./dev-data-{port}"));
    let relay = std::env::var("COMMONS_RELAY").unwrap_or_else(|_| "http://127.0.0.1:8787".into());
    let kdf = if std::env::var("COMMONS_FAST_KDF").is_ok() { commons_core::vault::KdfParams::TEST } else { commons_core::vault::KdfParams::STRONG };
    let app = App::with_kdf(data.into(), &relay, kdf);
    app.start_background();
    let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind");
    eprintln!("dev bridge on http://127.0.0.1:{port}  (ui: {})", ui.display());
    for conn in listener.incoming().flatten() {
        let (app, ui) = (app.clone(), ui.clone());
        std::thread::spawn(move || handle(conn, app, ui));
    }
}
