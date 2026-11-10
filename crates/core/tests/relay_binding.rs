//! Audit finding: a signed relay request used to be valid at every relay the device is registered on. A request signed for a relay that
//! says it binds is good only there; older clients and relays (no binding) keep working both ways.
mod common;
use common::*;
use commons_core::transport::*;
use commons_proto::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A TCP pass-through that remembers the bytes of every request that came through.
fn recorder(target: String) -> (String, Arc<Mutex<Vec<Vec<u8>>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    std::thread::spawn(move || {
        for c in l.incoming().flatten() {
            let (target, seen) = (target.clone(), s2.clone());
            std::thread::spawn(move || {
                let mut up = TcpStream::connect(&target).unwrap();
                let mut c2 = c.try_clone().unwrap();
                let mut up2 = up.try_clone().unwrap();
                std::thread::spawn(move || { let mut b = [0u8; 65536]; while let Ok(n) = up2.read(&mut b) { if n == 0 || c2.write_all(&b[..n]).is_err() { break; } } });
                let mut c = c;
                let mut b = [0u8; 65536];
                // One entry per connection, the whole of what was sent: a request's headers and body can arrive in separate reads.
                let slot = { let mut s = seen.lock().unwrap(); s.push(Vec::new()); s.len() - 1 };
                while let Ok(n) = c.read(&mut b) { if n == 0 { break; } seen.lock().unwrap()[slot].extend_from_slice(&b[..n]); if up.write_all(&b[..n]).is_err() { break; } }
            });
        }
    });
    (format!("http://{addr}"), seen)
}

fn register(base: &str, dev: &ed25519_dalek::SigningKey, cert: &DeviceCert) {
    let t = HttpTransport::new(base);
    let r = t.request(Some(dev), "POST", "/v1/devices", &serde_json::to_vec(&RegisterReq { cert: cert.clone(), invite: None }).unwrap(), Duration::from_secs(5)).unwrap();
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
}
fn raw(addr: &str, bytes: &[u8]) -> u16 {
    let mut s = TcpStream::connect(addr.trim_start_matches("http://")).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    s.write_all(bytes).unwrap();
    let mut out = vec![0u8; 12];
    s.read_exact(&mut out).unwrap();
    String::from_utf8_lossy(&out[9..12]).parse().unwrap()
}

#[test]
fn a_captured_request_is_good_only_at_the_relay_it_was_made_for() {
    let (na, nb) = (Net::new(), Net::new());
    let acct = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let dev = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let cert = DeviceCert::issue(&acct, &dev.verifying_key().to_bytes());
    let (via, seen) = recorder(na.url().trim_start_matches("http://").to_string());
    // The device is registered on both relays (it moved, or it was added to a second one).
    register(&na.url(), &dev, &cert);
    register(&nb.url(), &dev, &cert);
    // A real client request to relay A, through the recorder.
    let gid = "a".repeat(32);
    let body = serde_json::to_vec(&AclReq { add: vec![cert.account.clone()], ..Default::default() }).unwrap();
    let path = format!("/v1/groups/{gid}/acl");
    let t = HttpTransport::new(&via);
    assert_eq!(t.request(Some(&dev), "POST", &path, &body, Duration::from_secs(5)).unwrap().status, 200);
    let captured = seen.lock().unwrap().iter().find(|r| r.starts_with(format!("POST {path}").as_bytes())).cloned().expect("the request was seen");
    let text = String::from_utf8_lossy(&captured).to_lowercase();
    assert!(text.contains("x-commons-auth2:") && !text.contains("\r\nx-commons-auth:"), "a bound relay gets only the bound header");
    // The same bytes, played at relay A again (same relay: still accepted within the window, as documented) and at relay B (refused).
    assert_ne!(raw(&na.url(), &captured), 401, "same relay");
    assert_eq!(raw(&nb.url(), &captured), 401, "another relay refuses it");
}

#[test]
fn older_clients_keep_working_with_a_new_relay() {
    let n = Net::new();
    let acct = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let dev = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let cert = DeviceCert::issue(&acct, &dev.verifying_key().to_bytes());
    // An older client signs the old way: a new relay accepts it.
    let body = serde_json::to_vec(&RegisterReq { cert: cert.clone(), invite: None }).unwrap();
    let h = sign_request(&dev, "POST", "/v1/devices", &body);
    let r = ureq::post(&format!("{}/v1/devices", n.url())).set(AUTH_HEADER, &h).send_bytes(&body).unwrap();
    assert_eq!(r.status(), 200);
    // The same old-style header also works for any other request.
    let acl = serde_json::to_vec(&AclReq { add: vec![cert.account.clone()], ..Default::default() }).unwrap();
    let h = sign_request(&dev, "POST", "/v1/groups/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/acl", &acl);
    let r = ureq::post(&format!("{}/v1/groups/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/acl", n.url())).set(AUTH_HEADER, &h).send_bytes(&acl).unwrap();
    assert_eq!(r.status(), 200);
}

#[test]
fn a_relay_that_gets_a_new_id_does_not_lock_its_clients_out() {
    let n = Net::new();
    let acct = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let dev = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let cert = DeviceCert::issue(&acct, &dev.verifying_key().to_bytes());
    let t = HttpTransport::new(&n.url());
    register(&n.url(), &dev, &cert);
    let acl = serde_json::to_vec(&AclReq { add: vec![cert.account.clone()], ..Default::default() }).unwrap();
    let path = "/v1/groups/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/acl";
    assert_eq!(t.request(Some(&dev), "POST", path, &acl, Duration::from_secs(5)).unwrap().status, 200);
    // The relay's database is replaced by one with another id (a restore, a rebuild): the client's remembered id is stale.
    let db = rusqlite::Connection::open(n.db_path()).unwrap();
    db.execute("UPDATE relay_meta SET v='00000000000000000000000000000001' WHERE k='id'", []).unwrap();
    assert_eq!(t.request(Some(&dev), "POST", path, &acl, Duration::from_secs(5)).unwrap().status, 200, "it asked again and signed for the new id");
}
