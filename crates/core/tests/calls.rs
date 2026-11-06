//! Stage 2: call tokens and call keys.
//! * the relay signs a LiveKit token only for a current member, with a room name that does not reveal the group id;
//! * a removed member is refused tokens at once (while its read access is still in the grace period);
//! * the media keys come from the MLS group: equal for members, per sender, rotated by every membership change,
//!   and a removed member's keys do not match the new ones.
mod common;
use common::*;
use commons_proto::*;
use commons_relay::LiveKitCfg;
use std::collections::BTreeMap;

const SECRET: &str = "unit-test-secret-0123456789abcdef-0123456789";

fn net_with_calls() -> Net {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let data = tempfile::tempdir().unwrap();
    let cfg = LiveKitCfg { api_key: "k1".into(), api_secret: SECRET.into(), url: "wss://calls.example.test".into(), token_ttl_secs: 600 };
    let relay = rt.block_on(commons_relay::start_with("127.0.0.1:0".parse().unwrap(), data.path().join("relay"), Some(cfg))).unwrap();
    let addr = relay.addr;
    Net { rt, relay: Some(relay), data, addr }
}

#[derive(serde::Deserialize, Debug)]
struct Grant { room: String, #[serde(rename = "roomJoin")] room_join: bool, #[serde(rename = "canPublish")] can_publish: bool }
#[derive(serde::Deserialize, Debug)]
struct Claims { iss: String, sub: String, exp: u64, nbf: u64, video: Grant }

fn keys(p: &Peer, gid: &str) -> Result<(u64, BTreeMap<String, [u8; 32]>), String> {
    p.c.call_state(gid).map(|s| (s.epoch, s.keys)).map_err(|e| e.to_string())
}

#[test]
fn tokens_only_for_current_members_and_reveal_no_group_id() {
    let net = net_with_calls();
    let (mut a, mut b, mut c) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"), Peer::new(&net, "carol"));
    let (_cid, gid) = { let mut r: Vec<&mut Peer> = vec![&mut b]; community_with(&net, &mut a, &mut r) };
    let t = a.c.call_token(&gid).expect("member gets a token");
    assert_eq!(t.url, "wss://calls.example.test");
    // verify the signature with the shared secret and inspect the claims
    let mut v = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
    v.set_required_spec_claims(&["exp"]);
    let d = jsonwebtoken::decode::<Claims>(&t.token, &jsonwebtoken::DecodingKey::from_secret(SECRET.as_bytes()), &v).expect("valid HS256 token");
    assert_eq!(d.claims.iss, "k1");
    assert_eq!(d.claims.sub, a.c.my_device(), "identity is the device id and nothing else");
    assert!(d.claims.video.room_join && d.claims.video.can_publish);
    assert_eq!(d.claims.video.room, call_room(&gid));
    assert_ne!(d.claims.video.room, gid, "media server must not see the group id");
    assert!(!d.claims.video.room.contains(&gid[..8]));
    assert!(d.claims.exp - d.claims.nbf <= 600 + 30, "short lived");
    assert!(jsonwebtoken::decode::<Claims>(&t.token, &jsonwebtoken::DecodingKey::from_secret(b"another-secret-another-secret-1234"), &v).is_err(), "wrong secret must not verify");
    assert!(!String::from_utf8_lossy(&t.token.as_bytes()).contains("club"), "no names in the token");
    // a registered stranger who is in no group of this relay user: refused
    assert!(c.c.call_token(&gid).is_err());
    // and straight against the relay, bypassing the client's own check
    let k = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let acct = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let cert = DeviceCert::issue(&acct, &k.verifying_key().to_bytes());
    let tr = commons_core::transport::HttpTransport::new(&net.url());
    use commons_core::transport::Transport;
    let d5 = std::time::Duration::from_secs(5);
    assert_eq!(tr.request(Some(&k), "POST", "/v1/devices", &serde_json::to_vec(&RegisterReq { cert }).unwrap(), d5).unwrap().status, 200);
    assert_eq!(tr.request(Some(&k), "POST", &format!("/v1/groups/{gid}/call-token"), b"{}", d5).unwrap().status, 403, "registered non-member");
    assert_eq!(tr.request(None, "POST", &format!("/v1/groups/{gid}/call-token"), b"{}", d5).unwrap().status, 401, "unsigned");
    // removal: refused immediately, although read access is still in its grace period
    a.c.kick(&_cid, &b.acct()).unwrap();
    let mut all: Vec<&mut Peer> = vec![&mut a, &mut b]; settle(&mut all);
    let bk = b.c.call_token(&gid);
    assert!(bk.is_err(), "removed member must not get a token: {bk:?}");
    let bk2 = tr.request(Some(&ed25519_dalek::SigningKey::from_bytes(&hex::decode(&b.c.st.device_secret).unwrap().try_into().unwrap())), "POST", &format!("/v1/groups/{gid}/call-token"), b"{}", d5).unwrap();
    assert_eq!(bk2.status, 403, "relay itself refuses the removed member before the read-ACL grace period ends");
    assert!(a.c.call_token(&gid).is_ok(), "remaining member still gets tokens");
}

#[test]
fn relay_without_call_server_says_so() {
    let net = Net::new();
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (_cid, gid) = { let mut r: Vec<&mut Peer> = vec![&mut b]; community_with(&net, &mut a, &mut r) };
    let e = a.c.call_token(&gid).unwrap_err().to_string();
    assert!(e.contains("no call server"), "{e}");
}

#[test]
fn call_keys_come_from_the_group_and_rotate_with_membership() {
    let net = net_with_calls();
    let (mut a, mut b, mut c) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"), Peer::new(&net, "carol"));
    let (cid, gid) = { let mut r: Vec<&mut Peer> = vec![&mut b, &mut c]; community_with(&net, &mut a, &mut r) };
    let (ea, ka) = keys(&a, &gid).unwrap(); let (eb, kb) = keys(&b, &gid).unwrap(); let (ec, kc) = keys(&c, &gid).unwrap();
    assert_eq!((ea, &ka), (eb, &kb)); assert_eq!((ea, &ka), (ec, &kc), "all members derive identical keys");
    assert_eq!(ka.len(), 3);
    let vals: std::collections::BTreeSet<_> = ka.values().collect();
    assert_eq!(vals.len(), 3, "every sender has its own key");
    assert!(ka.values().all(|k| k.iter().any(|b| *b != 0)));
    // stable while nothing changes
    assert_eq!(keys(&a, &gid).unwrap(), (ea, ka.clone()));
    // a membership change rotates everything
    a.c.kick(&cid, &c.acct()).unwrap();
    let mut all: Vec<&mut Peer> = vec![&mut a, &mut b, &mut c]; settle(&mut all);
    let (ea2, ka2) = keys(&a, &gid).unwrap(); let (eb2, kb2) = keys(&b, &gid).unwrap();
    assert!(ea2 > ea); assert_eq!((ea2, &ka2), (eb2, &kb2));
    assert_eq!(ka2.len(), 2);
    for (dev, k) in &ka2 { assert_ne!(ka.get(dev), Some(k), "key of a remaining sender changed with the epoch"); }
    // the removed member cannot obtain the new keys: it either errors or still holds the old ones
    match keys(&c, &gid) {
        Err(_) => {}
        Ok((ec2, kc2)) => { assert!(ec2 < ea2, "stale epoch"); for (dev, k) in &kc2 { assert_ne!(ka2.get(dev), Some(k)); } }
    }
    // adding someone also rotates
    let mut d = Peer::new(&net, "dave");
    let link = a.c.create_invite(&cid, Some(1), 3600).unwrap(); d.c.join_with_link(&link).unwrap();
    let mut all: Vec<&mut Peer> = vec![&mut a, &mut b, &mut d]; settle(&mut all);
    let (ea3, ka3) = keys(&a, &gid).unwrap(); let (ed3, kd3) = keys(&d, &gid).unwrap();
    assert!(ea3 > ea2); assert_eq!((ea3, &ka3), (ed3, &kd3)); assert_eq!(ka3.len(), 3);
}
