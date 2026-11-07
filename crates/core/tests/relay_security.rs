//! The relay must refuse what it should refuse, whatever a client (or attacker) sends.
mod common;
use common::*;
use commons_core::transport::*;
use commons_core::vault::{KdfParams, Vault};
use commons_proto::*;
use ed25519_dalek::SigningKey;
use std::time::Duration;

fn t(net: &Net) -> HttpTransport { HttpTransport::new(&net.url()) }
const D: Duration = Duration::from_secs(5);

fn register(net: &Net) -> (SigningKey, SigningKey, DeviceCert) {
    let acct = SigningKey::generate(&mut rand::rngs::OsRng);
    let dev = SigningKey::generate(&mut rand::rngs::OsRng);
    let cert = DeviceCert::issue(&acct, &dev.verifying_key().to_bytes());
    let r = t(net).request(Some(&dev), "POST", "/v1/devices", &serde_json::to_vec(&RegisterReq { cert: cert.clone(), invite: None }).unwrap(), D).unwrap();
    assert_eq!(r.status, 200);
    (acct, dev, cert)
}
fn post(net: &Net, k: &SigningKey, gid: &str, data: &[u8]) -> u16 {
    let body = serde_json::to_vec(&PostReq { id: hex::encode(rand::random::<[u8; 16]>()), data: b64(data) }).unwrap();
    t(net).request(Some(k), "POST", &format!("/v1/groups/{gid}/messages"), &body, D).unwrap().status
}

#[test]
fn relay_enforces_auth_membership_and_limits() {
    let net = Net::new();
    let (_a1, d1, c1) = register(&net);
    let (_a2, d2, _c2) = register(&net);
    let gid = hex::encode(rand::random::<[u8; 16]>());
    // creator claims a fresh group; a stranger cannot post or read it
    let acl = serde_json::to_vec(&AclReq::default()).unwrap();
    assert_eq!(t(&net).request(Some(&d1), "POST", &format!("/v1/groups/{gid}/acl"), &acl, D).unwrap().status, 200);
    assert_eq!(post(&net, &d1, &gid, &[1, 2, 3]), 200);
    assert_eq!(post(&net, &d2, &gid, &[1, 2, 3]), 403, "non-member may not post");
    assert_eq!(t(&net).request(Some(&d2), "POST", &format!("/v1/groups/{gid}/acl"), &serde_json::to_vec(&AclReq { add: vec![c1.account.clone()], ..Default::default() }).unwrap(), D).unwrap().status, 403, "non-member may not edit the ACL");
    let sync = serde_json::to_vec(&SyncReq { cursors: [(gid.clone(), 0)].into(), ..Default::default() }).unwrap();
    let r: SyncResp = serde_json::from_slice(&t(&net).request(Some(&d2), "POST", "/v1/sync", &sync, D).unwrap().body).unwrap();
    assert!(r.messages.is_empty() && r.denied == vec![gid.clone()], "stranger gets nothing, only a denial");
    let r: SyncResp = serde_json::from_slice(&t(&net).request(Some(&d1), "POST", "/v1/sync", &sync, D).unwrap().body).unwrap();
    assert_eq!(r.messages[&gid].len(), 1);

    // unauthenticated, bad signature, tampered body, replayed on another path, stale timestamp
    assert_eq!(t(&net).request(None, "POST", &format!("/v1/groups/{gid}/messages"), b"{}", D).unwrap().status, 401);
    let body = serde_json::to_vec(&PostReq { id: hex::encode(rand::random::<[u8; 16]>()), data: b64(&[9]) }).unwrap();
    let hdr = sign_request(&d1, "POST", &format!("/v1/groups/{gid}/messages"), &body);
    let mut bad = body.clone(); bad[5] ^= 1;
    assert!(verify_request(&hdr, "POST", &format!("/v1/groups/{gid}/messages"), &body).is_ok());
    assert!(verify_request(&hdr, "POST", &format!("/v1/groups/{gid}/messages"), &bad).is_err());
    assert!(verify_request(&hdr, "POST", "/v1/sync", &body).is_err());
    assert!(verify_request(&hdr, "GET", &format!("/v1/groups/{gid}/messages"), &body).is_err());
    let old = format!("{}.{}.{}", hex::encode(d1.verifying_key().to_bytes()), now_secs() - 3600, "00".repeat(64));
    assert!(verify_request(&old, "POST", "/x", b"").is_err());

    // a device must be registered with a cert signed by its account
    let rogue = SigningKey::generate(&mut rand::rngs::OsRng);
    let other_acct = SigningKey::generate(&mut rand::rngs::OsRng);
    let mut forged = DeviceCert::issue(&other_acct, &rogue.verifying_key().to_bytes());
    forged.account = c1.account.clone(); // claim to be device of account 1
    assert_eq!(t(&net).request(Some(&rogue), "POST", "/v1/devices", &serde_json::to_vec(&RegisterReq { cert: forged, invite: None }).unwrap(), D).unwrap().status, 400);
    assert_eq!(post(&net, &rogue, &gid, &[1]), 403, "unregistered device");

    // size limits and quotas
    assert_eq!(post(&net, &d1, &gid, &vec![1u8; MAX_MESSAGE_BYTES + 10]), 413);
    let blob = vec![0u8; 1000];
    let id = hex::encode(rand::random::<[u8; 16]>());
    assert_eq!(t(&net).request(Some(&d1), "PUT", &format!("/v1/blobs/{id}"), &blob, D).unwrap().status, 200);
    assert_eq!(t(&net).request(Some(&d1), "PUT", &format!("/v1/blobs/{id}"), &blob, D).unwrap().status, 409, "blobs are write-once");
    assert_eq!(t(&net).request(None, "GET", &format!("/v1/blobs/{id}"), &[], D).unwrap().status, 401);
    assert_eq!(t(&net).request(Some(&d1), "GET", "/v1/blobs/../../etc/passwd", &[], D).unwrap().status, 404);
    assert_eq!(t(&net).request(Some(&d1), "GET", &format!("/v1/blobs/{}", "g".repeat(32)), &[], D).unwrap().status, 400);

    // revocation: signed by the account key only, and then the device is locked out
    let (a3, d3, c3) = register(&net);
    let sig = sign_revocation(&a3, &unhex32(&c3.device).unwrap());
    let evil = RevokeReq { account: c1.account.clone(), device: c3.device.clone(), sig: sig.clone() };
    assert_eq!(t(&net).request(None, "POST", "/v1/devices/revoke", &serde_json::to_vec(&evil).unwrap(), D).unwrap().status, 400, "wrong account cannot revoke");
    let good = RevokeReq { account: c3.account.clone(), device: c3.device.clone(), sig };
    assert_eq!(t(&net).request(None, "POST", "/v1/devices/revoke", &serde_json::to_vec(&good).unwrap(), D).unwrap().status, 200);
    assert_eq!(post(&net, &d3, &gid, &[1]), 403);
    // key packages are taken once and the pool is bounded
    let kp = PublishKpReq { packages: vec![b64(&[1, 2, 3]); 150], last_resort: None };
    for _ in 0..3 { assert_eq!(t(&net).request(Some(&d1), "POST", "/v1/keypackages", &serde_json::to_vec(&kp).unwrap(), D).unwrap().status, 200); }
    assert_eq!(t(&net).request(Some(&d1), "POST", "/v1/keypackages", &serde_json::to_vec(&kp).unwrap(), D).unwrap().status, 429);
}

#[test]
fn relay_only_listens_on_loopback_in_tests() {
    let net = Net::new();
    assert!(net.addr.ip().is_loopback());
}

#[test]
fn vault_is_encrypted_atomic_and_falls_back_to_backup() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("state.cmns");
    let v = Vault::create(&p, "hunter2hunter2", KdfParams::TEST).unwrap();
    v.save(b"generation one: SECRET-MARKER").unwrap();
    v.save(b"generation two: SECRET-MARKER").unwrap();
    let raw = std::fs::read(&p).unwrap();
    assert!(!raw.windows(13).any(|w| w == b"SECRET-MARKER"));
    assert_eq!(Vault::open(&p, "hunter2hunter2").unwrap().1, b"generation two: SECRET-MARKER");
    assert!(matches!(Vault::open(&p, "nope").err().unwrap().downcast_ref::<commons_core::vault::VaultError>(), Some(commons_core::vault::VaultError::WrongPassphrase)));
    // torn write: main file damaged -> previous generation is used
    std::fs::write(&p, &raw[..raw.len() / 2]).unwrap();
    assert_eq!(Vault::open(&p, "hunter2hunter2").unwrap().1, b"generation one: SECRET-MARKER");
    // flipped bit anywhere is detected
    let mut flipped = raw.clone(); let n = flipped.len() - 5; flipped[n] ^= 1;
    std::fs::write(&p, flipped).unwrap(); std::fs::remove_file(p.with_extension("bak")).unwrap();
    assert!(Vault::open(&p, "hunter2hunter2").is_err());
}

#[test]
fn an_unauthenticated_revocation_of_someone_elses_device_wipes_nothing() {
    // Review H2: a signature by ANY key over a device id deleted that device's key packages and Welcomes.
    let net = Net::new();
    let (_va, vdev, vcert) = register(&net);
    let kp = serde_json::to_vec(&PublishKpReq { packages: vec![b64(&[1, 2, 3])], last_resort: Some(b64(&[4, 5, 6])) }).unwrap();
    assert_eq!(t(&net).request(Some(&vdev), "POST", "/v1/keypackages", &kp, D).unwrap().status, 200);
    let count = || -> i64 {
        let c = rusqlite::Connection::open_with_flags(net.db_path(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        c.query_row("SELECT COUNT(*) FROM keypackages WHERE device=?1", [&vcert.device], |r| r.get(0)).unwrap()
    };
    assert_eq!(count(), 2);
    let attacker = SigningKey::generate(&mut rand::rngs::OsRng);
    let dev_bytes = unhex32(&vcert.device).unwrap();
    let req = RevokeReq { account: hex::encode(attacker.verifying_key().to_bytes()), device: vcert.device.clone(), sig: sign_revocation(&attacker, &dev_bytes) };
    let r = t(&net).request(None, "POST", "/v1/devices/revoke", &serde_json::to_vec(&req).unwrap(), D).unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(count(), 2, "the victim's key packages must survive a revocation that does not match the device's account");
    assert_eq!(post(&net, &vdev, "00000000000000000000000000000001", &[1]), 403, "victim still authenticates (403 = not a member, not 'revoked')");
}

#[test]
fn unauthenticated_revocations_are_rate_limited() {
    let net = Net::with_limits(commons_relay::Limits { revokes_per_window: 3, ..Default::default() });
    let attacker = SigningKey::generate(&mut rand::rngs::OsRng);
    let req = RevokeReq { account: hex::encode(attacker.verifying_key().to_bytes()), device: "ab".repeat(32), sig: sign_revocation(&attacker, &[0xab; 32]) };
    let body = serde_json::to_vec(&req).unwrap();
    let codes: Vec<u16> = (0..5).map(|_| t(&net).request(None, "POST", "/v1/devices/revoke", &body, D).unwrap().status).collect();
    assert_eq!(codes, vec![200, 200, 200, 429, 429]);
}

#[test]
fn idle_unauthenticated_uploads_do_not_hold_the_upload_slots() {
    // Review M3: four sockets that announce a big body and then say nothing used up every upload slot.
    use std::io::Write;
    let net = Net::new();
    let (_a, dev, _c) = register(&net);
    let mut idle = vec![];
    for _ in 0..8 {
        let mut s = std::net::TcpStream::connect(net.addr).unwrap();
        s.write_all(b"PUT /v1/blobs/00000000000000000000000000000001 HTTP/1.1\r\nHost: x\r\nContent-Length: 20000000\r\n\r\n").unwrap();
        idle.push(s);
    }
    std::thread::sleep(Duration::from_millis(300));
    let id = hex::encode(rand::random::<[u8; 16]>());
    let r = t(&net).request(Some(&dev), "PUT", &format!("/v1/blobs/{id}"), b"hello blob", Duration::from_secs(5)).expect("upload must not wait behind idle sockets");
    assert_eq!(r.status, 200);
    drop(idle);
}

#[test]
fn an_upload_body_that_never_arrives_gives_the_slot_back() {
    use std::io::{Read, Write};
    let net = Net::with_limits(commons_relay::Limits { big_body_timeout: Duration::from_millis(500), ..Default::default() });
    let (_a, dev, _c) = register(&net);
    let hdr = sign_request(&dev, "PUT", "/v1/blobs/00000000000000000000000000000001", b"");
    let mut s = std::net::TcpStream::connect(net.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(s, "PUT /v1/blobs/00000000000000000000000000000001 HTTP/1.1\r\nHost: x\r\n{}: {}\r\nContent-Length: 20000000\r\n\r\n", AUTH_HEADER, hdr).unwrap();
    let mut out = String::new();
    let mut buf = [0u8; 512];
    let n = s.read(&mut buf).expect("the relay must answer instead of waiting forever");
    out.push_str(&String::from_utf8_lossy(&buf[..n]));
    assert!(out.starts_with("HTTP/1.1 408"), "{out}");
}

#[test]
fn hex_keys_have_one_spelling() {
    let k = "ab".repeat(32);
    assert!(unhex32(&k).is_ok());
    assert!(unhex32(&k.to_uppercase()).is_err());
}
