//! A voice room is an ordinary room whose entry in the community's table carries one extra word: `"kind":"voice"`.
//!
//! That word lives in the signed control group, not in an envelope, so an app from before voice rooms reads the
//! same bytes as an ordinary room and shows it as one. These tests pin that down, plus the RPC and snapshot the
//! sidebar uses.
mod common;
use common::*;
use commons_core::envelope::ROOM_VOICE;

#[test]
fn a_voice_room_is_a_room_with_a_kind() {
    let net = Net::new();
    let (mut alice, mut bob) = (Peer::new(&net, "alice"), Peer::new(&net, "bob"));
    let (cid, gen) = community_with(&net, &mut alice, &mut [&mut bob]);
    let lobby = alice.c.create_channel_kind(&cid, "Lounge", ROOM_VOICE).unwrap();
    settle(&mut [&mut alice, &mut bob]);
    let kinds = |p: &Peer| p.c.channels_of(&cid).into_iter().map(|c| (c.name, c.kind)).collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(kinds(&alice)["general"], "", "the room made with the community is a text room");
    assert_eq!(kinds(&alice)["lounge"], ROOM_VOICE);
    assert_eq!(kinds(&bob), kinds(&alice), "every member sees the kind");
    // A voice room is a real room: it has its own MLS group, so its call has keys like any other.
    assert_eq!(bob.c.group_accounts(&lobby).len(), 2);
    // Text is what an app from before voice rooms asks for by sending nothing; "text" means the same.
    let plain = alice.c.create_channel_kind(&cid, "plain", "text").unwrap();
    settle(&mut [&mut alice, &mut bob]);
    assert_eq!(bob.c.channels_of(&cid).iter().find(|c| c.gid == plain).unwrap().kind, "");
    // It is still just a room: admins make it, members do not, and unknown kinds are refused.
    assert!(bob.c.create_channel_kind(&cid, "stage", ROOM_VOICE).is_err());
    assert!(alice.c.create_channel_kind(&cid, "stage", "stage").is_err());
    // Nothing new went over any envelope: an old client on the same relay sees no notice at all.
    let ns = notices(&bob);
    assert!(!ns.iter().any(|n| n.contains("newer version") || n.contains("does not know")), "{ns:?}");
    // The room still works as a room (its call is the point, but text is allowed too).
    alice.c.send_text(&lobby, "hello from the lounge", None).unwrap();
    settle(&mut [&mut alice, &mut bob]);
    assert!(has_text(&bob, &lobby, "hello from the lounge"));
    let _ = gen;
}

#[test]
fn the_ui_creates_a_voice_room_and_sees_its_kind() {
    let net = Net::new();
    let dir = tempfile::tempdir().unwrap();
    let app = commons_core::api::App::with_kdf(dir.path().to_path_buf(), &net.url(), commons_core::vault::KdfParams::TEST);
    app.call("create_account", serde_json::json!({"name": "A", "passphrase": "correct horse battery", "relay": net.url()})).unwrap();
    let cid = app.call("create_community", serde_json::json!({"name": "club"})).unwrap()["id"].as_str().unwrap().to_string();
    let snap = app.call("snapshot", serde_json::json!({})).unwrap();
    let general = &snap["communities"][0]["channels"][0];
    assert_eq!(general["kind"], "", "a room from before voice rooms has no kind");
    // What the + dialog sends for a voice room, and what an older app sends (no kind) for a text room.
    let voice = app.call("create_channel", serde_json::json!({"cid": cid, "name": "Lounge", "kind": "voice"})).unwrap()["id"].as_str().unwrap().to_string();
    let text = app.call("create_channel", serde_json::json!({"cid": cid, "name": "Offtopic"})).unwrap()["id"].as_str().unwrap().to_string();
    let snap = app.call("snapshot", serde_json::json!({})).unwrap();
    let chans = snap["communities"][0]["channels"].as_array().unwrap();
    let kind = |gid: &str| chans.iter().find(|c| c["gid"] == gid).unwrap()["kind"].as_str().unwrap().to_string();
    assert_eq!(kind(&voice), ROOM_VOICE);
    assert_eq!(kind(&text), "");
    assert_eq!(chans.iter().filter(|c| c["kind"] == ROOM_VOICE).count(), 1);
}
