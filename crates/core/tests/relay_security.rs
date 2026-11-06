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
    let r = t(net).request(Some(&dev), "POST", "/v1/devices", &serde_json::to_vec(&RegisterReq { cert: cert.clone() }).unwrap(), D).unwrap();
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
    let acl = serde_json::to_vec(&AclReq { add: vec![], remove: vec![] }).unwrap();
    assert_eq!(t(&net).request(Some(&d1), "POST", &format!("/v1/groups/{gid}/acl"), &acl, D).unwrap().status, 200);
    assert_eq!(post(&net, &d1, &gid, &[1, 2, 3]), 200);
    assert_eq!(post(&net, &d2, &gid, &[1, 2, 3]), 403, "non-member may not post");
    assert_eq!(t(&net).request(Some(&d2), "POST", &format!("/v1/groups/{gid}/acl"), &serde_json::to_vec(&AclReq { add: vec![c1.account.clone()], remove: vec![] }).unwrap(), D).unwrap().status, 403, "non-member may not edit the ACL");
    let sync = serde_json::to_vec(&SyncReq { cursors: [(gid.clone(), 0)].into(), ack_welcomes: vec![], wait_ms: 0 }).unwrap();
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
    assert_eq!(t(&net).request(Some(&rogue), "POST", "/v1/devices", &serde_json::to_vec(&RegisterReq { cert: forged }).unwrap(), D).unwrap().status, 400);
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
