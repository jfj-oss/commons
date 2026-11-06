//! Development bridge: serves the UI over http://127.0.0.1 and exposes the same `rpc` the Tauri shell has.
//! Used for headless UI checks and for trying the app in two browser tabs without Tauri. Loopback only.
//!   COMMONS_DATA=/tmp/alice COMMONS_RELAY=http://127.0.0.1:8787 dev_bridge 7001 ../../app/ui
use commons_core::api::{bridge_request_ok, App};
use std::io::Read;

const CSP: &str = "Content-Security-Policy: default-src 'self'; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; script-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'; object-src 'none'";

fn main() {
    let mut args = std::env::args().skip(1);
    let port: u16 = args.next().and_then(|p| p.parse().ok()).unwrap_or(7001);
    let ui = std::path::PathBuf::from(args.next().unwrap_or_else(|| "app/ui".into()));
    let data = std::env::var("COMMONS_DATA").unwrap_or_else(|_| format!("./dev-data-{port}"));
    let relay = std::env::var("COMMONS_RELAY").unwrap_or_else(|_| "http://127.0.0.1:8787".into());
    let kdf = if std::env::var("COMMONS_FAST_KDF").is_ok() { commons_core::vault::KdfParams::TEST } else { commons_core::vault::KdfParams::STRONG };
    let app = App::with_kdf(data.into(), &relay, kdf);
    app.start_background();
    let server = tiny_http::Server::http(("127.0.0.1", port)).expect("bind");
    eprintln!("dev bridge on http://127.0.0.1:{port}  (ui: {})", ui.display());
    for mut req in server.incoming_requests() {
        let header = |name: &'static str| req.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str().to_string());
        if !bridge_request_ok(header("Host").as_deref(), header("Origin").as_deref(), port) {
            let _ = req.respond(tiny_http::Response::empty(403));
            continue;
        }
        let url = req.url().to_string();
        if req.method() == &tiny_http::Method::Post && url == "/rpc" {
            let mut body = String::new();
            let _ = req.as_reader().take(40 << 20).read_to_string(&mut body);
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let method = v["method"].as_str().unwrap_or("").to_string();
            let out = match app.call(&method, v["args"].clone()) { Ok(r) => serde_json::json!({"ok": r}), Err(e) => serde_json::json!({"err": e}) };
            let _ = req.respond(tiny_http::Response::from_string(out.to_string()).with_header("Content-Type: application/json".parse::<tiny_http::Header>().unwrap()));
        } else {
            let path = if url == "/" { "index.html".to_string() } else { url.trim_start_matches('/').split('?').next().unwrap_or("").to_string() };
            if path.contains("..") { let _ = req.respond(tiny_http::Response::empty(400)); continue; }
            match std::fs::read(ui.join(&path)) {
                Ok(b) => {
                    let ct = if path.ends_with(".css") { "text/css" } else if path.ends_with(".js") { "text/javascript" } else if path.ends_with(".svg") { "image/svg+xml" } else { "text/html; charset=utf-8" };
                    let _ = req.respond(tiny_http::Response::from_data(b).with_header(format!("Content-Type: {ct}").parse::<tiny_http::Header>().unwrap()).with_header(CSP.parse::<tiny_http::Header>().unwrap()));
                }
                Err(_) => { let _ = req.respond(tiny_http::Response::empty(404)); }
            }
        }
    }
}
