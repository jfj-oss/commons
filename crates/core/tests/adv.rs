//! The tester's attacks, as assertions: each of these worked before it was fixed.
mod common;
use common::*;
use commons_core::transport::*;
use commons_proto::*;
use ed25519_dalek::SigningKey;
use std::time::Duration;
const D: Duration = Duration::from_secs(20);
fn t(net: &Net) -> HttpTransport { HttpTransport::new(&net.url()) }
fn reg(net: &Net) -> (SigningKey, DeviceCert) {
    let acct = SigningKey::generate(&mut rand::rngs::OsRng);
    let dev = SigningKey::generate(&mut rand::rngs::OsRng);
    let cert = DeviceCert::issue(&acct, &dev.verifying_key().to_bytes());
    let r = t(net).request(Some(&dev), "POST", "/v1/devices", &serde_json::to_vec(&RegisterReq { cert: cert.clone(), invite: None }).unwrap(), D).unwrap();
    assert_eq!(r.status, 200); (dev, cert)
}
fn post(net: &Net, k: &SigningKey, gid: &str, data: &[u8]) -> u16 {
    let body = serde_json::to_vec(&PostReq { id: hex::encode(rand::random::<[u8; 16]>()), data: b64(data) }).unwrap();
    t(net).request(Some(k), "POST", &format!("/v1/groups/{gid}/messages"), &body, D).unwrap().status
}
fn dk(p: &Peer) -> SigningKey { p.c.debug_dev_key().unwrap() }

#[test]
fn unsolicited_channel_welcomes_are_refused() {
    let net = Net::new();
    let mut m = Peer::new(&net, "mallory"); let mut v = Peer::new(&net, "victim");
    m.tick(); v.tick();
    let cid = m.c.create_community("evil").unwrap();
    settle(&mut [&mut m, &mut v]);
    let ch = m.c.channels_of(&cid)[0].gid.clone();
    m.c.debug_add_account_devices(&ch, &v.acct()).unwrap();
    settle(&mut [&mut m, &mut v]);
    assert!(v.c.st.groups.is_empty(), "a room of an unknown community stays out: {:?}", notices(&v));
}

#[test]
fn a_member_cannot_touch_the_relay_acl() {
    let net = Net::new();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    a.tick(); b.tick();
    let cid = a.c.create_community("Club").unwrap();
    let link = a.c.create_invite(&cid, Some(1), 3600).unwrap();
    b.c.join_with_link(&link).unwrap();
    settle(&mut [&mut a, &mut b]);
    let ch = a.c.channels_of(&cid)[0].gid.clone();
    let k = dk(&b);
    let body = serde_json::to_vec(&AclReq { remove: vec![a.acct()], ..Default::default() }).unwrap();
    for g in [&cid, &ch] { assert_eq!(t(&net).request(Some(&k), "POST", &format!("/v1/groups/{g}/acl"), &body, D).unwrap().status, 403); }
    b.c.send_text(&ch, "alice can read this", None).unwrap();
    settle(&mut [&mut a, &mut b]);
    assert!(has_text(&a, &ch, "alice can read this"));
}

#[test]
fn an_admin_cannot_remove_the_owner_and_a_demoted_admin_has_no_say() {
    use commons_core::state::Role;
    let net = Net::new();
    let (mut a, mut b, mut c) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"), Peer::new(&net, "carol"));
    a.tick(); b.tick(); c.tick();
    let (cid, _) = community_with(&net, &mut a, &mut [&mut b, &mut c]);
    a.c.set_role(&cid, &b.acct(), Role::Admin).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    let grant = a.c.st.groups.contains_key(&cid).then(|| b.c.debug_my_grant(&cid)).flatten().expect("bob holds a grant");
    let k = dk(&b);
    let req = |remove: String| serde_json::to_vec(&AclReq { remove: vec![remove], grant: Some(grant.clone()), ..Default::default() }).unwrap();
    let path = format!("/v1/groups/{cid}/acl");
    assert_eq!(t(&net).request(Some(&k), "POST", &path, &req(a.acct()), D).unwrap().status, 403, "the owner stays");
    // an admin's power is real: and a cut-off member is put back by the owner's check
    assert_eq!(t(&net).request(Some(&k), "POST", &path, &req(c.acct()), D).unwrap().status, 200);
    settle(&mut [&mut a, &mut b, &mut c]);
    assert!(notices(&a).iter().any(|n| n.contains("let them back in")), "{:?}", notices(&a));
    assert!(!c.c.st.groups[&cid].denied, "carol is back");
    // once demoted, the old grant is worth nothing even though it is still a valid signature
    a.c.set_role(&cid, &b.acct(), Role::Member).unwrap();
    settle(&mut [&mut a, &mut b, &mut c]);
    assert_eq!(t(&net).request(Some(&k), "POST", &path, &req(c.acct()), D).unwrap().status, 403);
}

#[test]
fn junk_join_requests_do_not_shut_out_a_real_one() {
    let net = Net::new();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    a.tick(); b.tick();
    let cid = a.c.create_community("Club").unwrap();
    let link = a.c.create_invite(&cid, Some(1), 3600).unwrap();
    a.tick();
    // one device is stopped quickly; many devices are stopped by the group limit
    let (mal, _) = reg(&net);
    let codes: Vec<u16> = (0..15).map(|i| { let mut d = vec![TAG_JOIN_REQUEST]; d.extend(format!("junk{i}").bytes()); post(&net, &mal, &cid, &d) }).collect();
    assert_eq!(codes.iter().filter(|c| **c == 200).count(), 10);
    assert!(codes[10..].iter().all(|c| *c == 429));
    for n in 0..4 { let (m, _) = reg(&net); for i in 0..10 { let mut d = vec![TAG_JOIN_REQUEST]; d.extend(format!("junk{n}-{i}").bytes()); post(&net, &m, &cid, &d); } }
    b.c.join_with_link(&link).unwrap();
    settle(&mut [&mut a, &mut b]);
    assert!(b.c.st.communities.get(&cid).is_some_and(|c| !c.removed && !c.channels.is_empty()), "{:?}", notices(&b));
    assert!(a.c.st.groups[&cid].requests.is_empty(), "junk never queued");
    assert!(notices(&a).iter().any(|n| n.contains("malformed join")), "and it says so");
}

#[test]
fn big_junk_entries_do_not_wedge_the_group() {
    let net = Net::new();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    a.tick(); b.tick();
    let cid = a.c.create_community("Club").unwrap();
    let link = a.c.create_invite(&cid, Some(1), 3600).unwrap();
    b.c.join_with_link(&link).unwrap();
    settle(&mut [&mut a, &mut b]);
    let ch = a.c.channels_of(&cid)[0].gid.clone();
    let k = dk(&b);
    let mut too_big = vec![TAG_MLS]; too_big.extend(vec![7u8; MAX_MESSAGE_BYTES]);
    assert_eq!(post(&net, &k, &ch, &too_big), 413, "entries over the limit are refused");
    let mut junk = vec![TAG_MLS]; junk.extend(vec![7u8; 250 * 1024]);
    for _ in 0..150 { assert_eq!(post(&net, &k, &ch, &junk), 200); } // about 50 MB encoded: twice what a client reads at once
    let mut peek = SyncReq::default(); peek.cursors.insert(ch.clone(), 0);
    let r = t(&net).request(Some(&dk(&a)), "POST", "/v1/sync", &serde_json::to_vec(&peek).unwrap(), D).unwrap();
    assert!(r.body.len() as u64 <= SYNC_MAX_BYTES + 512 * 1024, "response is capped: {}", r.body.len());
    assert!(serde_json::from_slice::<SyncResp>(&r.body).unwrap().more.contains(&ch), "and says there is more");
    for _ in 0..30 { a.tick(); if a.c.st.groups[&ch].cursor >= 150 { break; } }
    a.c.send_text(&ch, "alice tries to talk", None).unwrap();
    for _ in 0..30 { a.tick(); b.tick(); if has_text(&b, &ch, "alice tries to talk") { break; } }
    assert!(has_text(&b, &ch, "alice tries to talk"), "{:?}", notices(&a).iter().rev().take(4).collect::<Vec<_>>());
    assert_eq!(a.c.outbox_len(), 0);
}

#[test]
fn a_cut_short_answer_is_retried_smaller() {
    let net = Net::new();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    a.tick(); b.tick();
    let cid = a.c.create_community("Club").unwrap();
    let link = a.c.create_invite(&cid, Some(1), 3600).unwrap();
    b.c.join_with_link(&link).unwrap();
    settle(&mut [&mut a, &mut b]);
    let ch = a.c.channels_of(&cid)[0].gid.clone();
    let k = dk(&b);
    let mut junk = vec![TAG_MLS]; junk.extend(vec![7u8; 40 * 1024]);
    for _ in 0..30 { post(&net, &k, &ch, &junk); }
    b.c.send_text(&ch, "after the junk", None).unwrap();
    for _ in 0..3 { b.tick(); }
    // this connection breaks anything over 300 KB, so only smaller answers get through
    a.faults.lock().unwrap().cut_sync_at = Some(300 * 1024);
    for _ in 0..40 { a.tick(); if has_text(&a, &ch, "after the junk") { break; } }
    assert!(has_text(&a, &ch, "after the junk"), "{:?}", notices(&a).iter().rev().take(4).collect::<Vec<_>>());
}

#[test]
fn acl_requests_are_bounded() {
    let net = Net::new();
    let (m, _) = reg(&net);
    let gid = hex::encode(rand::random::<[u8; 16]>());
    let add: Vec<String> = (0..MAX_ACL_BATCH + 1).map(|_| hex::encode(rand::random::<[u8; 32]>())).collect();
    let body = serde_json::to_vec(&AclReq { add, ..Default::default() }).unwrap();
    assert_eq!(t(&net).request(Some(&m), "POST", &format!("/v1/groups/{gid}/acl"), &body, D).unwrap().status, 413);
    // only uploads may be big, and the refusal comes before anything is stored
    let r = t(&net).request(Some(&m), "POST", "/v1/keypackages", &vec![b'a'; 6 << 20], D);
    assert!(r.map(|x| x.status == 413).unwrap_or(true));
}

#[test]
fn a_skewed_clock_is_named() {
    let net = Net::new();
    let (m, _) = reg(&net);
    let body = b"{}".to_vec();
    let ts = now_secs() - 700;
    let hdr = { use ed25519_dalek::Signer; let mut mm = b"commons/req/v1".to_vec(); mm.extend_from_slice(&sha256(&[b"POST", b"/v1/sync", &ts.to_be_bytes(), &body])); format!("{}.{}.{}", hex::encode(m.verifying_key().to_bytes()), ts, hex::encode(m.sign(&mm).to_bytes())) };
    match ureq::post(&format!("{}/v1/sync", net.url())).set("x-commons-auth", &hdr).send_bytes(&body) {
        Err(ureq::Error::Status(401, r)) => assert!(r.into_string().unwrap().starts_with("clock skew")),
        other => panic!("{:?}", other.map(|r| r.status())),
    }
}

#[test]
fn the_rendezvous_needs_an_account_and_keeps_its_first_writer() {
    let net = Net::new();
    let (k1, _) = reg(&net); let (k2, _) = reg(&net);
    let rid = hex::encode(rand::random::<[u8; 16]>());
    let path = format!("/v1/rendezvous/{rid}/a");
    assert_eq!(t(&net).request(None, "PUT", &path, b"x", D).unwrap().status, 401, "strangers cannot write");
    assert_eq!(t(&net).request(Some(&k1), "PUT", &path, b"sealed", D).unwrap().status, 200);
    assert_eq!(t(&net).request(Some(&k2), "PUT", &path, b"evil", D).unwrap().status, 409);
    assert_eq!(t(&net).request(None, "GET", &path, &[], D).unwrap().body, b"sealed");
}

#[test]
fn a_member_cannot_revoke_someone_elses_device() {
    // Review H1: a revocation signed by the SENDER's account over a stranger's device was accepted by every member.
    use commons_core::envelope::Envelope;
    let net = Net::new();
    let (mut a, mut b, mut m) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"), Peer::new(&net, "mallory"));
    a.tick(); b.tick(); m.tick();
    let (cid, _gen) = community_with(&net, &mut a, &mut [&mut b, &mut m]);
    let bob_dev = b.c.my_device();
    let sig = m.c.revocation_sig(&bob_dev).unwrap(); // mallory's account key over bob's device
    m.c.debug_send_envelope(&cid, Envelope::Revoked { account: m.acct(), device: bob_dev.clone(), sig }).unwrap();
    settle(&mut [&mut a, &mut b, &mut m]);
    assert!(!a.c.st.revoked.contains_key(&bob_dev), "alice must not treat bob's device as revoked: {:?}", notices(&a));
    assert!(!b.c.st.self_revoked, "bob must not be told his own device is revoked");
    a.c.send_text(&a.c.channels_of(&cid)[0].gid.clone(), "still here", None).unwrap();
    settle(&mut [&mut a, &mut b, &mut m]);
}
