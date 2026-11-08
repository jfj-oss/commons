//! Who is in a room's call, as seen from the rail.
//!
//! The relay asks LiveKit (the authoritative participant list) and answers with *device ids only*; each member
//! maps those to the people in its own MLS group. These tests pin down:
//!  * the relay asks LiveKit correctly (a server-API token scoped to the hashed room), and only for rooms the
//!    caller is a current member of, and not cut off from calls in;
//!  * names never travel: the answer holds device ids and the client adds names locally;
//!  * a join or a leave (including a crashed client, which LiveKit drops) shows up after the short cache;
//!  * the whole feature is invisible to older clients: it writes nothing to any room's log and produces no
//!    notices, so a 0.2.x client on the same relay sees exactly the bytes it saw before.
mod common;
use common::*;
use commons_core::vault::KdfParams;
use commons_proto::*;
use commons_relay::LiveKitCfg;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SECRET: &str = "presence-test-secret-0123456789abcdef";

#[derive(Default)]
struct Fake {
    /// room name -> [(identity, LiveKit participant state)]
    rooms: Mutex<HashMap<String, Vec<(String, String)>>>,
    /// every request the relay made to the media server: (path, authorization header)
    requests: Mutex<Vec<(String, String)>>,
}

fn fake_livekit() -> (u16, Arc<Fake>) {
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    let fake = Arc::new(Fake::default());
    let f = fake.clone();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let f = f.clone();
            std::thread::spawn(move || serve(s, f));
        }
    });
    (port, fake)
}

fn serve(mut s: TcpStream, fake: Arc<Fake>) {
    let mut buf = Vec::new();
    let end = loop {
        let mut tmp = [0u8; 4096];
        let n = match s.read(&mut tmp) { Ok(0) | Err(_) => return, Ok(n) => n };
        buf.extend_from_slice(&tmp[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") { break i + 4; }
        if buf.len() > 64 * 1024 { return; }
    };
    let head = String::from_utf8_lossy(&buf[..end]).to_string();
    let mut lines = head.lines();
    let path = lines.next().unwrap_or("").split_whitespace().nth(1).unwrap_or("").to_string();
    let (mut auth, mut len) = (String::new(), 0usize);
    for l in lines {
        let Some((name, value)) = l.split_once(':') else { continue };
        let value = value.trim();
        if name.eq_ignore_ascii_case("authorization") { auth = value.to_string(); }
        if name.eq_ignore_ascii_case("content-length") { len = value.parse().unwrap_or(0); }
    }
    let mut body = buf[end..].to_vec();
    while body.len() < len {
        let mut tmp = [0u8; 4096];
        match s.read(&mut tmp) { Ok(0) | Err(_) => break, Ok(n) => body.extend_from_slice(&tmp[..n]) }
    }
    fake.requests.lock().unwrap().push((path.clone(), auth));
    if !path.ends_with("/livekit.RoomService/ListParticipants") {
        let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
        return;
    }
    let room = serde_json::from_slice::<serde_json::Value>(&body).ok()
        .and_then(|v| v["room"].as_str().map(str::to_string)).unwrap_or_default();
    let parts = fake.rooms.lock().unwrap().get(&room).cloned().unwrap_or_default();
    let list: Vec<serde_json::Value> = parts.iter().map(|(id, st)| serde_json::json!({"identity": id, "state": st})).collect();
    let out = serde_json::json!({"participants": list}).to_string();
    let _ = s.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{out}", out.len()).as_bytes());
}

fn net_with(fake_url: &str) -> Net {
    let cfg = LiveKitCfg { api_key: "k1".into(), api_secret: SECRET.into(), url: fake_url.into(), token_ttl_secs: 600, turn_port: None };
    Net::with_limits(commons_relay::Limits { livekit: Some(cfg), ..Default::default() })
}

fn key(p: &Peer) -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&hex::decode(&p.c.st.device_secret).unwrap().try_into().unwrap())
}

fn ask(net: &Net, p: &Peer, gids: &[String]) -> commons_core::transport::HttpResp {
    use commons_core::transport::Transport;
    let body = serde_json::to_vec(&CallPresenceReq { gids: gids.to_vec() }).unwrap();
    commons_core::transport::HttpTransport::new(&net.url())
        .request(Some(&key(p)), "POST", "/v1/call-presence", &body, Duration::from_secs(5)).unwrap()
}

fn ask_ok(net: &Net, p: &Peer, gids: &[String]) -> CallPresenceResp {
    let r = ask(net, p, gids);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    serde_json::from_slice(&r.body).unwrap()
}

#[derive(serde::Deserialize, Debug)]
struct AdminGrant { room: String, #[serde(rename = "roomAdmin")] room_admin: bool }
#[derive(serde::Deserialize, Debug)]
struct AdminClaims { iss: String, sub: String, exp: u64, nbf: u64, video: AdminGrant }

#[test]
fn the_relay_asks_livekit_with_a_room_scoped_admin_token_and_returns_devices_only() {
    let (port, fake) = fake_livekit();
    let net = net_with(&format!("http://127.0.0.1:{port}"));
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (_cid, gid) = { let mut r: Vec<&mut Peer> = vec![&mut b]; community_with(&net, &mut a, &mut r) };
    fake.rooms.lock().unwrap().insert(call_room(&gid), vec![
        (a.c.my_device(), "ACTIVE".into()),
        (b.c.my_device(), "ACTIVE".into()),
        // not ours, or gone: neither is shown
        ("not-a-device-id".into(), "ACTIVE".into()),
        ("ab".repeat(32), "DISCONNECTED".into()),
    ]);
    let r = ask(&net, &a, &[gid.clone()]);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let body = String::from_utf8_lossy(&r.body).to_string();
    assert!(!body.contains("alice") && !body.contains("bob"), "names must never be in the answer: {body}");
    let resp: CallPresenceResp = serde_json::from_slice(&r.body).unwrap();
    let devices = resp.rooms.get(&gid).expect("the room has people");
    assert_eq!(devices.len(), 2);
    assert!(devices.contains(&a.c.my_device()) && devices.contains(&b.c.my_device()));
    assert!(!devices.contains(&"ab".repeat(32)), "a disconnected participant is not in the call");
    // this device turns the ids into people using its own group, with no help from the relay
    let people = a.c.call_presence(&resp);
    let list = &people[&gid];
    assert_eq!(list.len(), 2);
    let by_acct: std::collections::BTreeMap<_, _> = list.iter().map(|p| (p.account.clone(), p)).collect();
    assert_eq!(by_acct[&a.acct()].name, "alice");
    assert!(by_acct[&a.acct()].is_me);
    assert_eq!(by_acct[&b.acct()].name, "bob");
    assert!(!by_acct[&b.acct()].is_me);
    assert_eq!(by_acct[&b.acct()].devices, vec![b.c.my_device()]);
    // the relay asked the media server with a short-lived server token for exactly this (hashed) room
    let reqs = fake.requests.lock().unwrap();
    let (_, auth) = reqs.iter().find(|(p, _)| p.ends_with("/ListParticipants")).expect("asked LiveKit");
    let token = auth.strip_prefix("Bearer ").expect("Bearer token");
    let mut v = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
    v.set_required_spec_claims(&["exp"]);
    let d = jsonwebtoken::decode::<AdminClaims>(token, &jsonwebtoken::DecodingKey::from_secret(SECRET.as_bytes()), &v).expect("valid HS256 server token");
    assert_eq!(d.claims.iss, "k1");
    assert_eq!(d.claims.sub, "k1");
    assert!(d.claims.video.room_admin);
    assert_eq!(d.claims.video.room, call_room(&gid));
    assert_ne!(d.claims.video.room, gid, "LiveKit sees a hash, not the group id");
    assert!(d.claims.exp - d.claims.nbf <= 70, "short lived");
}

#[test]
fn only_current_members_who_are_not_cut_off_from_calls_learn_anything() {
    use commons_core::transport::Transport;
    let (port, fake) = fake_livekit();
    let net = net_with(&format!("http://127.0.0.1:{port}"));
    let (mut a, mut b, c) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"), Peer::new(&net, "carol"));
    let (cid, gid) = { let mut r: Vec<&mut Peer> = vec![&mut b]; community_with(&net, &mut a, &mut r) };
    fake.rooms.lock().unwrap().insert(call_room(&gid), vec![(a.c.my_device(), "ACTIVE".into()), (b.c.my_device(), "ACTIVE".into())]);
    // a registered stranger who is in no room of this relay gets an empty answer (and cannot tell that apart
    // from a room where nobody is calling)
    let r = ask_ok(&net, &c, &[gid.clone()]);
    assert!(r.rooms.get(&gid).is_none(), "a non-member learns nothing");
    assert_eq!(ask(&net, &c, &[gid.clone()]).status, 200, "and is not told that the room exists");
    // unsigned requests are refused
    let tr = commons_core::transport::HttpTransport::new(&net.url());
    let body = serde_json::to_vec(&CallPresenceReq { gids: vec![gid.clone()] }).unwrap();
    assert_eq!(tr.request(None, "POST", "/v1/call-presence", &body, Duration::from_secs(5)).unwrap().status, 401);
    // a member cut off from calls in that room learns nothing either, while read access remains
    let ak = key(&a);
    let req = serde_json::to_vec(&AclReq { call_remove: vec![b.acct()], ..Default::default() }).unwrap();
    assert_eq!(tr.request(Some(&ak), "POST", &format!("/v1/groups/{gid}/acl"), &req, Duration::from_secs(5)).unwrap().status, 200);
    assert!(b.c.call_token(&gid).is_err(), "cut off from calls");
    assert!(ask_ok(&net, &b, &[gid.clone()]).rooms.get(&gid).is_none(), "and cannot see who is in the call");
    assert!(ask_ok(&net, &a, &[gid.clone()]).rooms.contains_key(&gid), "others still can");
    // a removed member is refused as soon as membership goes, without waiting for the read grace period
    a.c.kick(&cid, &b.acct()).unwrap();
    let mut all: Vec<&mut Peer> = vec![&mut a, &mut b]; settle(&mut all);
    assert!(ask_ok(&net, &b, &[gid.clone()]).rooms.get(&gid).is_none(), "a removed member learns nothing");
}

#[test]
fn joins_and_leaves_show_up_after_the_short_cache() {
    let (port, fake) = fake_livekit();
    let net = net_with(&format!("http://127.0.0.1:{port}"));
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (_cid, gid) = { let mut r: Vec<&mut Peer> = vec![&mut b]; community_with(&net, &mut a, &mut r) };
    let devs = |r: &CallPresenceResp| r.rooms.get(&gid).cloned().unwrap_or_default();
    fake.rooms.lock().unwrap().insert(call_room(&gid), vec![(a.c.my_device(), "ACTIVE".into())]);
    assert_eq!(devs(&ask_ok(&net, &a, &[gid.clone()])), vec![a.c.my_device()]);
    // bob joins the call
    fake.rooms.lock().unwrap().get_mut(&call_room(&gid)).unwrap().push((b.c.my_device(), "ACTIVE".into()));
    let t = Instant::now();
    loop {
        if devs(&ask_ok(&net, &a, &[gid.clone()])).len() == 2 { break; }
        assert!(t.elapsed() < Duration::from_secs(8), "a join must show within a few seconds");
        std::thread::sleep(Duration::from_millis(250));
    }
    // bob's client crashes: LiveKit drops the participant, and the rail follows after the cache expires
    fake.rooms.lock().unwrap().get_mut(&call_room(&gid)).unwrap().retain(|(d, _)| d != &b.c.my_device());
    let t = Instant::now();
    loop {
        let now = devs(&ask_ok(&net, &a, &[gid.clone()]));
        if now.len() == 1 { assert_eq!(now, vec![a.c.my_device()]); break; }
        assert!(t.elapsed() < Duration::from_secs(8), "a crashed client must drop off within a short timeout");
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[test]
fn the_app_maps_device_ids_to_names_for_the_open_community() {
    let (port, fake) = fake_livekit();
    let net = net_with(&format!("http://127.0.0.1:{port}"));
    let dir = tempfile::tempdir().unwrap();
    let app = commons_core::api::App::with_kdf(dir.path().to_path_buf(), &net.url(), KdfParams::TEST);
    app.call("create_account", serde_json::json!({"name": "A", "passphrase": "correct horse battery", "relay": net.url()})).unwrap();
    let cid = app.call("create_community", serde_json::json!({"name": "club"})).unwrap()["id"].as_str().unwrap().to_string();
    let snap = app.call("snapshot", serde_json::json!({})).unwrap();
    let gid = snap["communities"][0]["channels"][0]["gid"].as_str().unwrap().to_string();
    let dev = snap["me"]["device"].as_str().unwrap().to_string();
    fake.rooms.lock().unwrap().insert(call_room(&gid), vec![(dev, "ACTIVE".into())]);
    let r = app.call("call_presence", serde_json::json!({"cid": cid})).unwrap();
    let list = r["rooms"][&gid].as_array().expect("the room is in the answer");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["name"], "A", "the name comes from this device's own state");
    assert_eq!(list[0]["me"], true);
    assert!(app.call("call_presence", serde_json::json!({"cid": "00".repeat(16)})).unwrap()["rooms"].as_object().unwrap().is_empty(), "not a community we know: nothing to ask");
    // a relay with no media server says so; the UI treats that as "no calls", not as an error page
    let plain = Net::new();
    let r = ask(&plain, &Peer::new(&plain, "dave"), &[gid]);
    assert_eq!(r.status, 404);
}

#[test]
fn presence_never_reaches_the_log_and_older_clients_see_nothing() {
    let (port, fake) = fake_livekit();
    let net = net_with(&format!("http://127.0.0.1:{port}"));
    let (mut a, mut b) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (cid, gid) = { let mut r: Vec<&mut Peer> = vec![&mut b]; community_with(&net, &mut a, &mut r) };
    fake.rooms.lock().unwrap().insert(call_room(&gid), vec![(a.c.my_device(), "ACTIVE".into()), (b.c.my_device(), "ACTIVE".into())]);
    settle(&mut [&mut a, &mut b]);
    let before_chan = relay_entries(&net, &gid).len();
    let before_ctrl = relay_entries(&net, &cid).len();
    let before_activity = b.c.activity;
    let before_notices = notices(&b);
    for _ in 0..3 { let _ = ask_ok(&net, &a, &[gid.clone(), cid.clone()]); }
    let _ = ask_ok(&net, &b, &[gid.clone()]);
    // The feature is a new HTTP endpoint, not a new kind of message: the room logs are byte-for-byte untouched,
    // so a 0.2.x client syncing the same relay has nothing to skip, no "newer version" notice and no activity.
    assert_eq!(relay_entries(&net, &gid).len(), before_chan, "no presence in a room's log");
    assert_eq!(relay_entries(&net, &cid).len(), before_ctrl, "nor in the community's log");
    let _ = b.c.tick(0);
    assert_eq!(b.c.activity, before_activity, "an older client sees no activity for presence");
    assert_eq!(notices(&b), before_notices);
    assert!(!notices(&b).iter().any(|n| n.contains("does not know") || n.contains("newer version")));
    // The wire formats old versions do exchange are exactly as they were.
    assert_eq!(commons_core::envelope::Envelope::Typing.to_bytes(), br#"{"t":"Typing"}"#);
    // A plaintext text still travels and lands as before (this is the path a 0.2.x client uses).
    a.c.send_text(&gid, "still here", None).unwrap();
    settle(&mut [&mut a, &mut b]);
    assert!(has_text(&b, &gid, "still here"));
}
