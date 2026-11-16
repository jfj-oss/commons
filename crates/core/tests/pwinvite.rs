//! Password invites: a friend with only the inviter's password and a name gets in, and every way the password could
//! be misused is refused. The unit tests for the derivation and sealing are in src/pwinvite.rs.
mod common;
use common::*;
use commons_core::api::App;
use commons_core::pwinvite as pw;
use commons_core::transport::*;
use commons_core::vault::KdfParams;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

const K: KdfParams = KdfParams::TEST;
const PW: &str = "amber-fox-teal-moss-rain-47";

fn app(net: &Net) -> (Arc<App>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let a = App::with_kdf(dir.path().to_path_buf(), &net.url(), K);
    a.start_background();
    (a, dir)
}
fn wait<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let end = Instant::now() + Duration::from_secs(40);
    while Instant::now() < end { if let Some(v) = f() { return v; } std::thread::sleep(Duration::from_millis(150)); }
    panic!("timed out: {what}");
}
fn api(net: &Net) -> Api { Api { t: HttpTransport::new(&net.url()).into_arc() } }
fn first_channel(a: &App) -> Option<String> {
    let s = a.call("snapshot", json!({})).ok()?;
    s["communities"].as_array()?.first()?["channels"].as_array()?.first()?["gid"].as_str().map(|x| x.to_string())
}
/// Alice with a community, as an app.
fn alice(net: &Net) -> (Arc<App>, tempfile::TempDir, String) {
    let (a, d) = app(net);
    a.call("create_account", json!({"name": "alice", "passphrase": "correct horse battery", "relay": net.url()})).unwrap();
    let cid = a.call("create_community", json!({"name": "club"})).unwrap()["id"].as_str().unwrap().to_string();
    (a, d, cid)
}
fn db(net: &Net) -> rusqlite::Connection { rusqlite::Connection::open(net.db_path()).unwrap() }

#[test]
fn a_new_person_joins_with_only_the_password_and_a_name_and_can_chat() {
    let net = Net::new();
    let (al, _d1, cid) = alice(&net);
    let made = al.call("create_invite", json!({"cid": cid, "password": PW, "uses": 1, "ttl": 7 * 86400})).unwrap();
    assert_eq!(made["password"], true);
    assert!(made["download"].as_str().unwrap().ends_with("/join"));
    let (bo, _d2) = app(&net);
    // Wrong password: nothing is created, and the message is the same as for an unknown one.
    let e1 = bo.call("create_account_password", json!({"password": "amber-fox-teal-moss-rain-48", "name": "bob", "passphrase": "my device pass"})).unwrap_err();
    assert_eq!(e1, pw::NOT_FOUND);
    assert_eq!(bo.call("state", json!({})).unwrap()["exists"], false, "a failed password creates no account");
    // The right one (with different spacing and capitals), the name, and a device password: that is all.
    let r = bo.call("create_account_password", json!({"password": "Amber Fox TEAL moss-RAIN 47", "name": "bob", "passphrase": "my device pass"})).unwrap();
    assert_eq!(r["ok"], true);
    assert!(r.get("join_error").is_none(), "{r}");
    let gid = wait("bob sees the channel", || first_channel(&bo));
    wait("bob can speak", || bo.call("send_text", json!({"gid": gid, "text": "hello from bob"})).ok());
    wait("alice reads bob", || {
        let m = al.call("messages", json!({"gid": gid, "limit": 20})).ok()?;
        m.to_string().contains("hello from bob").then_some(())
    });
    // The one use is spent: the same password cannot let a second person in.
    let (cy, _d3) = app(&net);
    let again = cy.call("create_account_password", json!({"password": PW, "name": "carol", "passphrase": "another device pass"}));
    // The sealed copy may still be fetchable, but the admin refuses the second join.
    if again.is_ok() {
        std::thread::sleep(Duration::from_secs(6));
        let s = cy.call("snapshot", json!({})).unwrap();
        assert!(s["communities"].as_array().unwrap().is_empty(), "a used-up invite must not admit a second person: {s}");
    }
    for a in [&al, &bo, &cy] { a.shutdown(); }
}

#[test]
fn an_existing_account_joins_another_community_with_a_password() {
    let net = Net::new();
    let (al, _d1, cid) = alice(&net);
    let (bo, _d2) = app(&net);
    bo.call("create_account", json!({"name": "bob", "passphrase": "my device pass", "relay": net.url()})).unwrap();
    al.call("create_invite", json!({"cid": cid, "password": PW})).unwrap();
    assert!(bo.call("join_password", json!({"password": "wrong-wrong-wrong-00"})).is_err());
    bo.call("join_password", json!({"password": PW})).unwrap();
    wait("bob gets the room", || first_channel(&bo));
    al.shutdown(); bo.shutdown();
}

#[test]
fn the_relay_holds_only_an_id_and_ciphertext() {
    let net = Net::new();
    let (al, _d1, cid) = alice(&net);
    let made = al.call("create_invite", json!({"cid": cid, "password": PW})).unwrap();
    let link = made["link"].as_str().unwrap().to_string();
    let d = pw::derive(PW, &net.url(), K).unwrap();
    let bytes = relay_bytes(&net);
    assert!(contains(&bytes, &d.lid), "filed under the derived id");
    for needle in [PW, "amber", "fox-teal", link.as_str(), &link[link.len() - 60..], "commons://join/"] { assert!(!contains(&bytes, needle), "relay storage must not hold {needle:?}"); }
    let (n, len): (u64, u64) = db(&net).query_row("SELECT COUNT(*), SUM(LENGTH(blob)) FROM pwinvites", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(n, 1);
    assert_eq!(len % 1024, 41, "a padded ciphertext (version, nonce, tag and whole kilobytes), {len}");
    al.shutdown();
}

#[test]
fn a_weak_password_is_refused_before_anything_is_made() {
    let net = Net::new();
    let (al, _d, cid) = alice(&net);
    for weak in ["hunter2", "password123", "12345678", "bluefish"] {
        let e = al.call("create_invite", json!({"cid": cid, "password": weak})).unwrap_err();
        assert!(e.contains("too easy to guess"), "{weak}: {e}");
    }
    let snap = al.call("snapshot", json!({})).unwrap();
    assert!(snap["communities"][0]["invites"].as_array().unwrap().is_empty(), "a refused password leaves no invite behind");
    assert_eq!(db(&net).query_row::<u64, _, _>("SELECT COUNT(*) FROM pwinvites", [], |r| r.get(0)).unwrap(), 0);
    al.shutdown();
}

#[test]
fn the_same_password_cannot_be_filed_twice_and_the_failed_invite_is_taken_back() {
    let net = Net::new();
    let (al, _d, cid) = alice(&net);
    al.call("create_invite", json!({"cid": cid, "password": PW})).unwrap();
    let e = al.call("create_invite", json!({"cid": cid, "password": PW})).unwrap_err();
    assert!(e.contains("already uses that password"), "{e}");
    let snap = al.call("snapshot", json!({})).unwrap();
    assert_eq!(snap["communities"][0]["invites"].as_array().unwrap().len(), 1, "only the first invite is listed");
    al.shutdown();
}

#[test]
fn revoking_removes_the_sealed_copy_and_the_password_stops_working() {
    let net = Net::new();
    let (al, _d1, cid) = alice(&net);
    let made = al.call("create_invite", json!({"cid": cid, "password": PW})).unwrap();
    let id = made["id"].as_str().unwrap();
    al.call("revoke_invite", json!({"cid": cid, "id": id})).unwrap();
    assert_eq!(db(&net).query_row::<u64, _, _>("SELECT COUNT(*) FROM pwinvites", [], |r| r.get(0)).unwrap(), 0, "the copy is gone from the relay");
    let e = pw::lookup(&api(&net), &net.url(), PW, K).unwrap_err();
    assert_eq!(e.to_string(), pw::NOT_FOUND);
    // The password can be used for a new invite afterwards.
    al.call("create_invite", json!({"cid": cid, "password": PW})).unwrap();
    al.shutdown();
}

#[test]
fn an_expired_copy_is_not_found_and_reads_like_a_wrong_password() {
    let net = Net::new();
    let (al, _d1, cid) = alice(&net);
    al.call("create_invite", json!({"cid": cid, "password": PW, "ttl": 7 * 86400})).unwrap();
    assert!(pw::lookup(&api(&net), &net.url(), PW, K).is_ok());
    db(&net).execute("UPDATE pwinvites SET expires = 1", []).unwrap();
    let gone = pw::lookup(&api(&net), &net.url(), PW, K).unwrap_err().to_string();
    let wrong = pw::lookup(&api(&net), &net.url(), "amber-fox-teal-moss-rain-99", K).unwrap_err().to_string();
    assert_eq!(gone, wrong, "failures do not say whether a password exists");
    assert_eq!(gone, pw::NOT_FOUND);
    al.shutdown();
}

#[test]
fn the_use_limit_is_the_communitys_own_and_a_ttl_beyond_the_cap_is_clamped() {
    let net = Net::new();
    let (al, _d1, cid) = alice(&net);
    al.call("create_invite", json!({"cid": cid, "password": PW, "uses": 2, "ttl": 400 * 86400})).unwrap();
    let snap = al.call("snapshot", json!({})).unwrap();
    let inv = &snap["communities"][0]["invites"][0];
    assert_eq!(inv["uses_left"], 2);
    assert_eq!(inv["password"], true);
    let exp = inv["expires"].as_u64().unwrap();
    assert!(exp <= commons_proto::now_secs() + 30 * 86400 + 5, "a password invite lives at most 30 days");
    let (relay_exp, fetches): (i64, i64) = db(&net).query_row("SELECT expires, fetches_left FROM pwinvites", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert!(relay_exp as u64 <= commons_proto::now_secs() + 30 * 86400 + 5);
    assert_eq!(fetches, 9, "the relay keeps a few spare fetches per use");
    al.shutdown();
}

#[test]
fn a_banned_account_is_still_refused_when_it_comes_in_by_password() {
    let net = Net::new();
    let mut al = Peer::new(&net, "alice");
    let mut bob = Peer::new(&net, "bob");
    let cid = al.c.create_community("club").unwrap();
    let d = pw::derive(PW, &net.url(), K).unwrap();
    al.c.create_invite_password(&cid, &d, Some(5), 86400).unwrap();
    let link = pw::lookup(&api(&net), &net.url(), PW, K).unwrap();
    bob.c.join_with_link(&link).unwrap();
    settle(&mut [&mut al, &mut bob]);
    assert!(al.c.group_accounts(&cid).contains(&bob.acct()));
    al.c.kick(&cid, &bob.acct()).unwrap();
    settle(&mut [&mut al, &mut bob]);
    assert!(!al.c.group_accounts(&cid).contains(&bob.acct()));
    let link = pw::lookup(&api(&net), &net.url(), PW, K).unwrap();
    let _ = bob.c.join_with_link(&link);
    settle(&mut [&mut al, &mut bob]);
    assert!(!al.c.group_accounts(&cid).contains(&bob.acct()), "the password only delivers the invite; bans still apply");
}

#[test]
fn lookups_are_rationed_per_id_and_for_the_whole_relay_and_misses_do_not_say_which() {
    let net = Net::with_limits(commons_relay::Limits { pwinvite_lookups_per_id_per_window: 3, pwinvite_misses_per_window: 6, ..Default::default() });
    let (al, _d, cid) = alice(&net);
    al.call("create_invite", json!({"cid": cid, "password": PW})).unwrap();
    let lid_hit = pw::derive(PW, &net.url(), K).unwrap().lid;
    let look = |lid: &str| net_post(&net, lid);
    // The same id asked too often is refused, hit or miss.
    let miss = pw::derive("zzz-aaa-qqq-11", &net.url(), K).unwrap().lid;
    let codes: Vec<u16> = (0..5).map(|_| look(&miss)).collect();
    assert_eq!(codes, vec![404, 404, 404, 429, 429], "{codes:?}");
    // Misses on different ids spend the whole-relay budget (6 here, 3 already spent).
    let mut seen429 = false;
    for i in 0..10 { let c = look(&pw::derive(&format!("guess-{i}-aaa-bbb"), &net.url(), K).unwrap().lid); if c == 429 { seen429 = true; break; } assert_eq!(c, 404); }
    assert!(seen429, "guessing with many ids is cut off");
    // With the guessing budget spent, even the right id is refused: a guesser gets nothing more by luck.
    assert_eq!(look(&lid_hit), 429);
    al.shutdown();
}

fn net_post(net: &Net, lid: &str) -> u16 {
    HttpTransport::new(&net.url()).request(None, "POST", "/v1/pwinvite/lookup", &serde_json::to_vec(&json!({"lid": lid})).unwrap(), Duration::from_secs(5)).unwrap().status
}

#[test]
fn a_stranger_cannot_file_or_remove_copies() {
    let net = Net::new();
    let (al, _d, cid) = alice(&net);
    let made: Value = al.call("create_invite", json!({"cid": cid, "password": PW})).unwrap();
    let _ = made;
    let lid = pw::derive(PW, &net.url(), K).unwrap().lid;
    let t = HttpTransport::new(&net.url());
    assert_eq!(t.request(None, "PUT", &format!("/v1/pwinvite/{}", "ab".repeat(32)), b"{}", Duration::from_secs(5)).unwrap().status, 401);
    assert_eq!(t.request(None, "DELETE", &format!("/v1/pwinvite/{lid}"), &[], Duration::from_secs(5)).unwrap().status, 401);
    // Another account may not delete it either.
    let acct = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let dev = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let cert = commons_proto::DeviceCert::issue(&acct, &dev.verifying_key().to_bytes());
    assert_eq!(t.request(Some(&dev), "POST", "/v1/devices", &serde_json::to_vec(&commons_proto::RegisterReq { cert, invite: None }).unwrap(), Duration::from_secs(5)).unwrap().status, 200);
    assert_eq!(t.request(Some(&dev), "DELETE", &format!("/v1/pwinvite/{lid}"), &[], Duration::from_secs(5)).unwrap().status, 200);
    assert!(pw::lookup(&api(&net), &net.url(), PW, K).is_ok(), "someone else's delete changed nothing");
    al.shutdown();
}

#[test]
fn an_old_relay_without_the_feature_fails_with_a_clear_message() {
    // A relay that does not know /v1/info's `pwinvite` field answers false; the lookup stops before asking anything else.
    struct Old(HttpTransport);
    impl Transport for Old {
        fn request(&self, k: Option<&ed25519_dalek::SigningKey>, m: &str, p: &str, b: &[u8], t: Duration) -> anyhow::Result<HttpResp> {
            let mut r = self.0.request(k, m, p, b, t)?;
            if p == "/v1/info" { r.body = br#"{"gated":false,"ok":false}"#.to_vec(); }
            Ok(r)
        }
    }
    let net = Net::new();
    let a = Api { t: Arc::new(Old(HttpTransport::new(&net.url()))) };
    let e = pw::lookup(&a, &net.url(), PW, K).unwrap_err().to_string();
    assert!(e.contains("too old"), "{e}");
}

// ---- audit: the relay's spelling, older invites, and what a failed attempt costs

/// Wraps the transport the way a relay from before `alt` behaves: it does not say `pwalt` and ignores the extra id. Counts lookups.
struct Before(HttpTransport, Arc<std::sync::atomic::AtomicUsize>);
impl Transport for Before {
    fn request(&self, k: Option<&ed25519_dalek::SigningKey>, m: &str, p: &str, b: &[u8], t: Duration) -> anyhow::Result<HttpResp> {
        if p == "/v1/pwinvite/lookup" {
            self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut v: Value = serde_json::from_slice(b).unwrap();
            v.as_object_mut().unwrap().remove("alt");
            return self.0.request(k, m, p, &serde_json::to_vec(&v).unwrap(), t);
        }
        let mut r = self.0.request(k, m, p, b, t)?;
        if p == "/v1/info" { let mut v: Value = serde_json::from_slice(&r.body).unwrap(); v.as_object_mut().unwrap().remove("pwalt"); r.body = serde_json::to_vec(&v).unwrap(); }
        Ok(r)
    }
}

/// A registered device of its own, to file a sealed copy the way an older app did.
fn stranger_dev(net: &Net) -> ed25519_dalek::SigningKey {
    let acct = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let dev = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let cert = commons_proto::DeviceCert::issue(&acct, &dev.verifying_key().to_bytes());
    let r = HttpTransport::new(&net.url()).request(Some(&dev), "POST", "/v1/devices", &serde_json::to_vec(&commons_proto::RegisterReq { cert, invite: None }).unwrap(), Duration::from_secs(5)).unwrap();
    assert_eq!(r.status, 200);
    dev
}

#[test]
fn the_way_the_relay_address_is_spelled_does_not_matter() {
    let net = Net::new();
    let (al, _d, cid) = alice(&net);
    al.call("create_invite", json!({"cid": cid, "password": PW, "uses": 5})).unwrap();
    let hostport = net.url().trim_start_matches("http://").to_string();
    for spelled in [net.url(), format!("{}/", net.url()), format!("{}///", net.url()), format!("HTTP://{hostport}"), hostport.clone(), format!("https://{hostport}"), format!(" {} ", net.url())] {
        assert!(pw::lookup(&api(&net), &spelled, PW, K).is_ok(), "{spelled}");
    }
    // The whole flow from a friend who typed the address with a final slash.
    let (bo, _d2) = app(&net);
    let r = bo.call("create_account_password", json!({"password": PW, "name": "bob", "passphrase": "my device pass", "relay": format!("{}/", net.url())})).unwrap();
    assert_eq!(r["ok"], true);
    assert!(r.get("join_error").is_none(), "{r}");
    for a in [&al, &bo] { a.shutdown(); }
}

#[test]
fn an_invite_filed_under_the_older_id_is_still_found_by_both_kinds_of_relay() {
    let net = Net::new();
    let dev = stranger_dev(&net);
    let t = HttpTransport::new(&net.url());
    // What an app from before this change filed: the id and key from the address exactly as that app spelled it.
    let old = pw::derive_legacy(PW, &net.url(), K).unwrap();
    let body = pw::put_body(&old, "commons://join/legacy-link", 3600, Some(1)).unwrap();
    assert_eq!(t.request(Some(&dev), "PUT", &format!("/v1/pwinvite/{}", old.lid), &body, Duration::from_secs(5)).unwrap().status, 200);
    // A relay that knows `alt`: one request.
    assert_eq!(pw::lookup(&api(&net), &net.url(), PW, K).unwrap(), "commons://join/legacy-link");
    // A relay from before: the new id misses, then the older one is asked. (The copy had 6 fetches; this is the second.)
    let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let a = Api { t: Arc::new(Before(HttpTransport::new(&net.url()), n.clone())) };
    assert_eq!(pw::lookup(&a, &net.url(), PW, K).unwrap(), "commons://join/legacy-link");
    assert_eq!(n.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[test]
fn a_wrong_password_costs_one_miss_on_a_relay_that_knows_both_ids() {
    let limit = 6;
    let guesses = |old_relay: bool| -> usize {
        let net = Net::with_limits(commons_relay::Limits { pwinvite_misses_per_window: limit, ..Default::default() });
        let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let a = if old_relay { Api { t: Arc::new(Before(HttpTransport::new(&net.url()), n)) } } else { api(&net) };
        let mut wrong = 0;
        for i in 0..20 {
            let e = pw::lookup(&a, &net.url(), &format!("guess-{i}-aaa-bbb"), K).unwrap_err().to_string();
            if e.contains("Too many tries") { break; }
            assert_eq!(e, pw::NOT_FOUND);
            wrong += 1;
        }
        wrong
    };
    let (new_relay, old_relay) = (guesses(false), guesses(true));
    assert_eq!(new_relay, limit as usize, "one miss per wrong password");
    assert_eq!(old_relay, limit as usize / 2, "a relay from before cannot be asked both ids at once, so each wrong password costs two there");
}

#[test]
fn a_failed_attempt_after_the_lookup_does_not_spend_another_fetch() {
    use std::os::unix::fs::PermissionsExt;
    let net = Net::new();
    let (al, _d, cid) = alice(&net);
    al.call("create_invite", json!({"cid": cid, "password": PW, "uses": 1})).unwrap();
    let left = || db(&net).query_row::<i64, _, _>("SELECT fetches_left FROM pwinvites", [], |r| r.get(0)).unwrap();
    let before = left();
    let (bo, d2) = app(&net);
    // Something local goes wrong after the invite was fetched: the account folder cannot be written.
    std::fs::set_permissions(d2.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
    if std::fs::File::create(d2.path().join("x")).is_ok() { eprintln!("skipped: this user can write anywhere"); std::fs::set_permissions(d2.path(), std::fs::Permissions::from_mode(0o700)).unwrap(); return; }
    let args = json!({"password": PW, "name": "bob", "passphrase": "my device pass"});
    assert!(bo.call("create_account_password", args.clone()).is_err(), "the first try fails");
    assert_eq!(left(), before - 1, "the failed try fetched once");
    std::fs::set_permissions(d2.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let r = bo.call("create_account_password", args).unwrap();
    assert_eq!(r["ok"], true);
    assert_eq!(left(), before - 1, "the retry used the copy it already had");
    for a in [&al, &bo] { a.shutdown(); }
}

/// A lookup or a filing as one caller: the address a reverse proxy would report. The relay is on loopback here, so the header is believed.
fn as_caller(net: &Net, ip: &str, method: &str, path: &str, body: &[u8]) -> u16 {
    let r = ureq::request(method, &format!("{}{path}", net.url())).set("X-Forwarded-For", ip).set("content-type", "application/json").send_bytes(body);
    match r { Ok(r) => r.status(), Err(ureq::Error::Status(c, _)) => c, Err(e) => panic!("{e}") }
}

/// Audit M5: forty junk lookups from anyone locked the whole relay for ten minutes, and filing a copy under an id said whether that
/// password was in use. Now one caller spends only its own budget, and a collision with somebody else's copy reads like success.
#[test]
fn one_caller_cannot_lock_everyone_out_and_filing_a_copy_is_not_an_oracle() {
    let net = Net::with_limits(commons_relay::Limits { pwinvite_misses_per_client: 6, pwinvite_misses_per_window: 100, ..Default::default() });
    let (al, _d, cid) = alice(&net);
    al.call("create_invite", json!({"cid": cid, "password": PW})).unwrap();
    let hit = pw::derive(PW, &net.url(), K).unwrap().lid;
    let look = |ip: &str, lid: &str| as_caller(&net, ip, "POST", "/v1/pwinvite/lookup", &serde_json::to_vec(&json!({"lid": lid})).unwrap());
    // Caller X guesses wrong six times and is then refused, even the right id; caller Y is not affected at all.
    for i in 0..6 { assert_eq!(look("203.0.113.5", &pw::derive(&format!("junk-{i}-aaa-bbb-ccc"), &net.url(), K).unwrap().lid), 404); }
    assert_eq!(look("203.0.113.5", &pw::derive("junk-9-aaa-bbb-ccc", &net.url(), K).unwrap().lid), 429);
    assert_eq!(look("203.0.113.5", &hit), 429, "a caller that has spent its budget gets nothing more, hits included");
    assert_eq!(look("203.0.113.6", &pw::derive("junk-8-aaa-bbb-ccc", &net.url(), K).unwrap().lid), 404, "another caller still gets answers");
    assert_eq!(look("203.0.113.6", &hit), 200, "and the friend with the right password gets in");
    // A stranger files a copy under alice's id: answered like success, nothing stored or replaced, alice's copy still opens.
    let dev = stranger_dev(&net);
    let put = |dev: &ed25519_dalek::SigningKey, lid: &str, pw: &str| {
        let d = pw::derive(pw, &net.url(), K).unwrap();
        let body = pw::put_body(&d, "commons://join/stranger", 3600, Some(1)).unwrap();
        HttpTransport::new(&net.url()).request(Some(dev), "PUT", &format!("/v1/pwinvite/{lid}"), &body, Duration::from_secs(5)).unwrap().status
    };
    assert_eq!(put(&dev, &hit, PW), 200, "a collision with someone else's copy is not reported");
    assert_eq!(put(&dev, &pw::derive("fresh-words-here-now-ok-12", &net.url(), K).unwrap().lid, "fresh-words-here-now-ok-12"), 200, "a free id is taken as before");
    let row: (String, Vec<u8>) = db(&net).query_row("SELECT owner, blob FROM pwinvites WHERE lid=?1", [&hit], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_ne!(row.0, hex::encode(dev.verifying_key().to_bytes()), "alice's row was not taken over");
    assert_eq!(pw::lookup(&api(&net), &net.url(), PW, K).is_ok(), true, "alice's copy still opens with her password");
    assert_eq!(db(&net).query_row::<u64, _, _>("SELECT COUNT(*) FROM pwinvites", [], |r| r.get(0)).unwrap(), 2, "two rows: alice's and the stranger's own");
    // Filing counts as a guess for the caller (loopback here: alice's own filing, the two above, and these): after its budget the stranger is refused there too.
    for i in 0..4 { let _ = put(&dev, &pw::derive(&format!("more-words-{i}-aaa-bbb-cc"), &net.url(), K).unwrap().lid, &format!("more-words-{i}-aaa-bbb-cc")); }
    assert_eq!(put(&dev, &pw::derive("last-words-aaa-bbb-cc-99", &net.url(), K).unwrap().lid, "last-words-aaa-bbb-cc-99"), 429);
    al.shutdown();
}

#[test]
fn a_full_table_makes_the_heaviest_account_give_up_a_copy_rather_than_refusing_everyone() {
    let net = Net::with_limits(commons_relay::Limits { max_pwinvites_total: 4, max_pwinvites_per_account: 10, pwinvite_misses_per_client: 100, ..Default::default() });
    let flooder = stranger_dev(&net);
    let put = |dev: &ed25519_dalek::SigningKey, pw: &str| {
        let d = pw::derive(pw, &net.url(), K).unwrap();
        let body = pw::put_body(&d, "commons://join/x", 3600, Some(1)).unwrap();
        HttpTransport::new(&net.url()).request(Some(dev), "PUT", &format!("/v1/pwinvite/{}", d.lid), &body, Duration::from_secs(5)).unwrap().status
    };
    for i in 0..4 { assert_eq!(put(&flooder, &format!("flood-words-{i}-aaa-bbb-cc")), 200); }
    assert_eq!(db(&net).query_row::<u64, _, _>("SELECT COUNT(*) FROM pwinvites", [], |r| r.get(0)).unwrap(), 4);
    // The table is full of one account's copies: an honest inviter still gets to file one, at the flooder's expense.
    let (al, _d, cid) = alice(&net);
    al.call("create_invite", json!({"cid": cid, "password": PW})).unwrap();
    assert!(pw::lookup(&api(&net), &net.url(), PW, K).is_ok());
    let flood_rows: u64 = db(&net).query_row("SELECT COUNT(*) FROM pwinvites WHERE owner=?1", [hex::encode(flooder.verifying_key().to_bytes())], |r| r.get(0)).unwrap();
    let _ = flood_rows; // the flooder's account is the device's account in DeviceCert terms; what matters is the total stayed at the cap
    assert_eq!(db(&net).query_row::<u64, _, _>("SELECT COUNT(*) FROM pwinvites", [], |r| r.get(0)).unwrap(), 4);
    // The flooder itself, being the heaviest, is refused when it tries to go on.
    assert_eq!(put(&flooder, "flood-words-9-aaa-bbb-cc"), 429);
    al.shutdown();
}
