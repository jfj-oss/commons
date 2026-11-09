//! The browser call page's bridge (callbridge.rs) must guard the sound-route methods exactly like the room methods:
//! no secret or a wrong secret is refused, a wrong Host is refused, and only the listed pcaudio_* names pass.
use commons_core::api::App;
use commons_core::callbridge::CallBridge;
use commons_core::vault::KdfParams;
use std::io::{Read, Write};
use std::net::TcpStream;

fn post(port: u16, host: &str, secret: Option<&str>, body: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let sec = secret.map(|x| format!("x-call-secret: {x}\r\n")).unwrap_or_default();
    write!(s, "POST /rpc HTTP/1.1\r\nHost: {host}\r\n{sec}Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
    let mut raw = vec![]; let _ = s.read_to_end(&mut raw);   // a refusal may reset the connection with the body unread
    let out = String::from_utf8_lossy(&raw).into_owned();
    let code = out.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    (code, out.split("\r\n\r\n").nth(1).unwrap_or("").to_string())
}

#[test]
fn sound_route_rpcs_need_the_one_time_secret() {
    let dir = std::env::temp_dir().join(format!("pcaudio-bridge-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::with_kdf(dir.clone(), "http://127.0.0.1:1", KdfParams::TEST);
    let b = CallBridge::start(app).unwrap();
    let url = b.grant("room1", "Room");
    let secret = url.split("#s=").nth(1).unwrap().split('&').next().unwrap().to_string();
    let host = format!("127.0.0.1:{}", b.port);
    let caps = r#"{"method":"pcaudio_caps","args":{}}"#;

    assert_eq!(post(b.port, &host, None, caps).0, 403, "no secret");
    assert_eq!(post(b.port, &host, Some("nope"), caps).0, 403, "wrong secret");
    assert!(matches!(post(b.port, "evil.example:80", Some(&secret), caps).0, 0 | 400), "wrong host");
    let (code, body) = post(b.port, &host, Some(&secret), caps);
    assert_eq!(code, 200, "{body}");
    assert!(body.contains("\"supported\""), "{body}");
    // names that merely start with pcaudio_, and other methods, are still refused
    assert_eq!(post(b.port, &host, Some(&secret), r#"{"method":"pcaudio_evil","args":{}}"#).0, 403);
    assert_eq!(post(b.port, &host, Some(&secret), r#"{"method":"snapshot","args":{}}"#).0, 403);
    // a bad stream id gets a refusal from the core, not a started program
    let (code, body) = post(b.port, &host, Some(&secret), r#"{"method":"pcaudio_start","args":{"id":"52; touch /tmp/x"}}"#);
    assert_eq!(code, 200); assert!(body.contains("\"err\""), "{body}");
    b.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
