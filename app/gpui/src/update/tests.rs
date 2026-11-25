//! update.rs tests. Every signature here is made with a throwaway key generated in the test; the real key is never read.
use super::*;
use ed25519_dalek::{Signer, SigningKey};
use std::sync::atomic::AtomicU64;

fn tmp(name: &str) -> PathBuf {
    let base = std::env::var_os("TMPDIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let d = base.join(format!("mega-update-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub struct TestKey { id: [u8; 8], signing: SigningKey, pub embedded: String }
impl TestKey {
    pub fn new(seed: u8) -> Self {
        use base64::Engine;
        let signing = SigningKey::from_bytes(&[seed; 32]);
        let id = [seed.wrapping_mul(7); 8];
        let mut bin = vec![0x45, 0x44];
        bin.extend_from_slice(&id);
        bin.extend_from_slice(signing.verifying_key().as_bytes());
        let e = base64::engine::general_purpose::STANDARD;
        let text = format!("untrusted comment: minisign public key: {}\n{}\n", id.iter().map(|b| format!("{b:02X}")).collect::<String>(), e.encode(&bin));
        Self { id, signing, embedded: e.encode(text.as_bytes()) }
    }
    /// Pre-hashed minisign signature with the version in the trusted comment, as `tauri signer sign --app-version` makes it.
    pub fn sign(&self, data: &[u8], version: &str) -> String {
        use base64::Engine;
        use blake2::Digest;
        let e = base64::engine::general_purpose::STANDARD;
        let h = blake2::Blake2b512::digest(data);
        let sig = self.signing.sign(&h).to_bytes();
        let mut bin1 = vec![0x45, 0x44];
        bin1.extend_from_slice(&self.id);
        bin1.extend_from_slice(&sig);
        let trusted = format!("timestamp:1\tfile:test\tprehashed\tversion:{version}");
        let mut global = sig.to_vec();
        global.extend_from_slice(trusted.as_bytes());
        let global = self.signing.sign(&global).to_bytes();
        let text = format!("untrusted comment: signature from minisign secret key\n{}\ntrusted comment: {trusted}\n{}\n", e.encode(&bin1), e.encode(global));
        e.encode(text.as_bytes())
    }
}

/// A file that passes the ELF check.
fn elf(tag: &str) -> Vec<u8> { let mut v = b"\x7fELF".to_vec(); v.extend_from_slice(tag.as_bytes()); v.resize(4096, 0); v }

#[test]
fn versions_compare_as_numbers() {
    use commons_core::update::version_lt;
    assert_eq!(version_lt("0.3.9", "0.3.10"), Some(true), "numbers, not text");
    assert_eq!(version_lt("0.3.11", "0.3.11"), Some(false));
    assert_eq!(version_lt("0.4.0", "0.3.99"), Some(false));
    assert_eq!(version_lt("1.0.0-beta", "1.0.0"), Some(true));
    assert_eq!(version_lt("0.3", "0.3.1"), None, "not a version");
}

#[test]
fn platform_keys_are_ours_alone() {
    assert_eq!(platform_key("linux", "x86_64").as_deref(), Some("linux-gpui-x86_64"));
    assert_eq!(platform_key("macos", "aarch64").as_deref(), Some("darwin-gpui-aarch64"));
    assert_eq!(platform_key("windows", "x86_64"), None);
    assert_eq!(platform_key("linux", "riscv64"), None);
    // none of the keys the Tauri apps and the browser versions look for
    for k in ["linux-x86_64", "darwin-aarch64", "windows-x86_64", "linux-browser-x86_64", "darwin-browser-aarch64", "windows-browser-x86_64"] {
        for (os, arch) in [("linux", "x86_64"), ("macos", "aarch64"), ("macos", "x86_64"), ("linux", "aarch64")] { assert_ne!(platform_key(os, arch).unwrap(), k); }
    }
    assert!(platform_key("linux", "x86_64").unwrap().contains("gpui"));
}

#[test]
fn the_trusted_key_is_the_tauri_updaters() {
    let k = public_key();
    assert!(!k.is_empty(), "the Tauri updater's public key is built in");
    let conf: Value = serde_json::from_str(include_str!("../../../src-tauri/tauri.conf.json")).unwrap();
    assert_eq!(conf["plugins"]["updater"]["pubkey"].as_str().unwrap(), k, "one key for every app (unless a test build replaced it)");
    // and it is a valid minisign public key
    use base64::Engine;
    let text = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(&k).unwrap()).unwrap();
    assert!(minisign_verify::PublicKey::decode(&text).is_ok());
}

fn manifest(v: &str, key: &str, sig: &str) -> Value {
    json!({"version": v, "notes": "  Faster rooms.\nA new look.  ", "platforms": {key: {"url": "update/mega-gpui", "signature": sig}, "linux-x86_64": {"url": "update/a.AppImage", "signature": "tauri"}}})
}

#[test]
fn only_a_newer_native_entry_is_offered() {
    let k = "linux-gpui-x86_64";
    let r = release_for(&manifest("0.3.12", k, "c2ln"), "0.3.11", k).unwrap().unwrap();
    assert_eq!((r.version.as_str(), r.notes.as_str(), r.required), ("0.3.12", "Faster rooms.\nA new look.", false));
    assert!(release_for(&manifest("0.3.11", k, "c2ln"), "0.3.11", k).unwrap().is_none(), "the same version is no update");
    assert!(release_for(&manifest("0.3.10", k, "c2ln"), "0.3.11", k).unwrap().is_none(), "no downgrades");
    assert!(release_for(&manifest("nonsense", k, "c2ln"), "0.3.11", k).unwrap().is_none());
    // the feed has the Tauri apps' entries but no native one yet: nothing, quietly
    let m = json!({"version": "0.3.12", "platforms": {"linux-x86_64": {"url": "update/a", "signature": "s"}, "linux-browser-x86_64": {"url": "update/b", "signature": "s"}}});
    assert!(release_for(&m, "0.3.11", k).unwrap().is_none(), "another app's entry is never taken for ours");
    assert!(release_for(&json!({"version": "0.3.12", "platforms": {k: {"url": "update/x"}}}), "0.3.11", k).unwrap_err().contains("signature"));
    assert!(release_for(&json!({"version": "0.3.12", "platforms": {k: {"signature": "s"}}}), "0.3.11", k).is_err());
    let mut m = manifest("0.3.12", k, "c2ln");
    m["minimum_version"] = json!("0.3.11");
    assert!(release_for(&m, "0.3.5", k).unwrap().unwrap().required);
}

#[test]
fn signature_accepts_only_the_right_file_key_and_version() {
    let key = TestKey::new(1);
    let other = TestKey::new(2);
    let data = elf("native");
    let ok = key.sign(&data, "0.3.12");
    assert!(verify(&data, &ok, &key.embedded, "0.3.12").is_ok());
    let mut bad = data.clone(); bad[100] ^= 1;
    assert!(verify(&bad, &ok, &key.embedded, "0.3.12").is_err(), "a changed byte");
    assert!(verify(&data, &other.sign(&data, "0.3.12"), &key.embedded, "0.3.12").is_err(), "another key");
    assert!(verify(&data, &ok, &other.embedded, "0.3.12").is_err(), "a build that trusts another key");
    assert!(verify(&data, &key.sign(&data, "0.3.10"), &key.embedded, "0.3.12").is_err(), "a genuine older file passed off as new");
    assert!(verify(&data, "", &key.embedded, "0.3.12").is_err(), "unsigned");
    assert!(verify(&data, "%%%", &key.embedded, "0.3.12").is_err(), "garbage");
    assert!(verify(&data, &ok, "", "0.3.12").is_err(), "no key at all");
    // the real public key refuses a throwaway-signed file
    assert!(verify(&data, &ok, &public_key(), "0.3.12").is_err() || public_key() == key.embedded);
}

#[test]
fn atomic_replace_keeps_the_previous_and_rolls_back() {
    let d = tmp("replace");
    let exe = d.join("mega-gpui");
    std::fs::write(&exe, elf("old")).unwrap();
    install_binary(&exe, &elf("new1")).unwrap();
    assert_eq!(std::fs::read(&exe).unwrap(), elf("new1"));
    assert_eq!(std::fs::read(previous_of(&exe)).unwrap(), elf("old"));
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; assert_eq!(std::fs::metadata(&exe).unwrap().permissions().mode() & 0o777, 0o755); }
    // a running process keeps the file it has open: replacing is by rename, so an open handle still reads the old data
    let open = std::fs::File::open(&exe).unwrap();
    install_binary(&exe, &elf("new2")).unwrap();
    let mut seen = Vec::new(); { use std::io::Read; let mut o = open; o.read_to_end(&mut seen).unwrap(); }
    assert_eq!(seen, elf("new1"), "the file the running program has open is not touched");
    assert_eq!(std::fs::read(previous_of(&exe)).unwrap(), elf("new1"), "the previous is the one before");
    rollback_binary(&exe).unwrap();
    assert_eq!(std::fs::read(&exe).unwrap(), elf("new1"));
    assert!(!previous_of(&exe).exists());
    assert!(rollback_binary(&exe).is_err(), "nothing left to go back to");
    // nothing but the program and its previous is left in the folder
    let mut names: Vec<String> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, vec!["mega-gpui".to_string()]);
}

#[test]
fn a_failed_install_leaves_the_old_program() {
    let d = tmp("failed");
    let exe = d.join("mega-gpui");
    std::fs::write(&exe, elf("old")).unwrap();
    assert!(install_binary(&exe, b"#!/bin/sh\necho hi\n").is_err() || !cfg!(target_os = "linux"), "not a program");
    assert!(install_binary(&exe, b"\x7fELF").is_err(), "too small");
    assert_eq!(std::fs::read(&exe).unwrap(), elf("old"));
    // a folder that cannot be written to
    let ro = d.join("ro");
    std::fs::create_dir(&ro).unwrap();
    std::fs::write(ro.join("mega-gpui"), elf("old")).unwrap();
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
        if std::fs::File::create(ro.join("probe")).is_err() {   // not running as root
            let e = install_binary(&ro.join("mega-gpui"), &elf("new")).unwrap_err();
            assert!(e.contains("writable"), "{e}");
            assert_eq!(std::fs::read(ro.join("mega-gpui")).unwrap(), elf("old"));
        }
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// A `.app.tar.gz` with the given entries: (path, bytes) files, "dir/" folders, ("path", "->target") links.
fn bundle_tar(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    {
        let mut b = tar::Builder::new(&mut gz);
        for (name, data) in entries {
            let mut h = tar::Header::new_gnu();
            let n = name.as_bytes();
            h.as_old_mut().name[..n.len()].copy_from_slice(n);   // written raw, so hostile paths can be forged
            if name.ends_with('/') { h.set_entry_type(tar::EntryType::Directory); h.set_size(0); h.set_mode(0o755); }
            else if let Some(t) = data.strip_prefix("->") { h.set_entry_type(tar::EntryType::Symlink); h.set_size(0); h.set_link_name(t).unwrap(); h.set_mode(0o777); }
            else { h.as_old_mut().linkflag[0] = b'0'; h.set_size(data.len() as u64); h.set_mode(0o755); }
            h.set_mtime(0); h.set_cksum();
            let body = if name.ends_with('/') || data.starts_with("->") { "" } else { data };
            b.append(&h, body.as_bytes()).unwrap();
        }
        b.finish().unwrap();
    }
    gz.finish().unwrap()
}

/// What codesign needs to accept a bundle on macOS (the test only checks the swap, not the program).
const PLIST: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>CFBundleExecutable</key><string>mega-gpui</string><key>CFBundleIdentifier</key><string>app.commons.chat.gpui.test</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>";

#[test]
fn a_bundle_is_swapped_and_can_go_back() {
    let d = tmp("bundle");
    let app = d.join("Megatron.app");
    std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
    std::fs::write(app.join("Contents/MacOS/mega-gpui"), "old").unwrap();
    let new = bundle_tar(&[("Megatron.app/", ""), ("Megatron.app/Contents/", ""), ("Megatron.app/Contents/MacOS/", ""), ("Megatron.app/Contents/Info.plist", PLIST), ("Megatron.app/Contents/MacOS/mega-gpui", "new"), ("Megatron.app/Contents/Resources/", ""), ("Megatron.app/Contents/Resources/link", "->../MacOS/mega-gpui")]);
    install_bundle(&app, &new).unwrap();
    assert_eq!(std::fs::read_to_string(app.join("Contents/MacOS/mega-gpui")).unwrap(), "new");
    assert_eq!(std::fs::read_to_string(d.join("Megatron.previous.app/Contents/MacOS/mega-gpui")).unwrap(), "old");
    assert_eq!(std::fs::read_to_string(app.join("Contents/Resources/link")).unwrap(), "new", "a link inside the bundle works");
    rollback_bundle(&app).unwrap();
    assert_eq!(std::fs::read_to_string(app.join("Contents/MacOS/mega-gpui")).unwrap(), "old");
    assert!(!d.join("Megatron.previous.app").exists());
    assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1, "no stage folder or bad copy left behind");
}

#[test]
fn a_hostile_archive_writes_nothing() {
    let d = tmp("hostile");
    let app = d.join("Megatron.app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(app.join("keep"), "old").unwrap();
    for (what, tgz) in [
        ("a path that climbs out", bundle_tar(&[("Megatron.app/../../evil", "x")])),
        ("an absolute path", bundle_tar(&[("/etc/evil", "x")])),
        ("a top level that is no app", bundle_tar(&[("evil", "x")])),
        ("two apps", bundle_tar(&[("A.app/x", "x"), ("B.app/x", "x")])),
        ("a link out of the bundle", bundle_tar(&[("Megatron.app/l", "->../../../etc/passwd")])),
        ("an absolute link", bundle_tar(&[("Megatron.app/l", "->/etc/passwd")])),
        ("an empty archive", bundle_tar(&[])),
        ("not an archive", b"nonsense".to_vec()),
    ] {
        assert!(install_bundle(&app, &tgz).is_err(), "{what} must be refused");
        assert_eq!(std::fs::read_to_string(app.join("keep")).unwrap(), "old", "{what}: the installed app is untouched");
        assert!(!d.parent().unwrap().join("evil").exists() && !d.join("evil").exists());
    }
    assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1, "no stage folder left behind");
}

#[test]
fn state_roundtrip_and_boot_counting() {
    let d = tmp("state");
    assert_eq!(State::load(&d), State::default());
    let s = State { whatsnew: Some(("0.3.12".into(), "Faster.\nNicer.".into())), boot: Some(("0.3.12".into(), 0)), rolled_back: None };
    s.save(&d);
    assert_eq!(State::load(&d), s);
    std::fs::write(d.join("gpui-update.json"), b"{broken").unwrap();
    assert_eq!(State::load(&d), State::default(), "a damaged file is no state");
    let mut st = s.clone();
    assert_eq!(boot_step(&mut st, "0.3.12"), Boot::Counted(1));
    assert_eq!(boot_step(&mut st, "0.3.12"), Boot::Counted(2));
    assert_eq!(boot_step(&mut st, "0.3.12"), Boot::RollBack("0.3.12".into()), "the third start in a row that never stayed up");
    assert_eq!((st.boot.clone(), st.rolled_back.clone(), st.whatsnew.clone()), (None, Some("0.3.12".into()), None));
    let mut st = s.clone();
    assert_eq!(boot_step(&mut st, "0.3.11"), Boot::Fine, "the old version starting is nothing to count");
    assert_eq!(st.boot, None);
    assert_eq!(boot_step(&mut State::default(), "0.3.12"), Boot::Fine);
}

/// The whole path against a real private relay serving a test feed: check, download, verify, install, state. It needs
/// the relay program, so it runs when `MEGA_TEST_RELAY_BIN` names one (tools/gpui-update-test.sh does that); the port is
/// `COMMONS_TEST_PORT_OFFSET` (1400+) above 20000.
#[test]
fn end_to_end_against_a_local_relay() {
    let Some(bin) = std::env::var_os("MEGA_TEST_RELAY_BIN") else { eprintln!("skipped: MEGA_TEST_RELAY_BIN not set"); return };
    let off: u16 = std::env::var("COMMONS_TEST_PORT_OFFSET").ok().and_then(|v| v.parse().ok()).unwrap_or(1400);
    let port = 20000 + off + 7;
    let d = tmp("e2e");
    let (site, data, install_dir) = (d.join("site"), d.join("relay-data"), d.join("install"));
    std::fs::create_dir_all(site.join("update")).unwrap();
    std::fs::create_dir_all(&install_dir).unwrap();
    let key = TestKey::new(9);
    let plat = platform().expect("a system with a native build");
    let new = elf("version-0.3.12");
    std::fs::write(site.join("update/Megatron_0.3.12_gpui"), &new).unwrap();
    let feed = |v: &str, sig: &str| json!({"version": v, "notes": "Faster rooms.\nA new look.", "pub_date": "2026-11-16T00:00:00Z", "platforms": {
        "linux-x86_64": {"url": "update/tauri.AppImage", "signature": "c2ln"}, plat.as_str(): {"url": "update/Megatron_0.3.12_gpui", "signature": sig}}});
    let write_feed = |v: Value| std::fs::write(site.join("update/latest.json"), serde_json::to_vec(&v).unwrap()).unwrap();
    write_feed(feed("0.3.12", &key.sign(&new, "0.3.12")));

    struct Relay(std::process::Child);
    impl Drop for Relay { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }
    let relay = Relay(std::process::Command::new(bin).args(["--bind", &format!("127.0.0.1:{port}"), "--data"]).arg(&data).arg("--site").arg(&site)
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().expect("start the relay"));
    let url = format!("http://127.0.0.1:{port}");
    for _ in 0..100 { if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() { break; } std::thread::sleep(Duration::from_millis(100)); }

    // 1. the feed names a newer version for this app: offered, with its notes
    let rel = check(&url, "0.3.11", &plat).unwrap().expect("an update is offered");
    assert_eq!((rel.version.as_str(), rel.notes.as_str()), ("0.3.12", "Faster rooms.\nA new look."));
    assert_eq!(rel.url, format!("{url}/update/Megatron_0.3.12_gpui"), "the address is made absolute on this relay");
    assert!(check(&url, "0.3.12", &plat).unwrap().is_none(), "already up to date");
    assert!(check(&url, "0.3.11", "darwin-gpui-x86_64").unwrap().is_none(), "no entry for another system");

    // 2. a good download is verified and replaces the program; the old one is kept
    let exe = install_dir.join("mega-gpui");
    std::fs::write(&exe, elf("version-0.3.11")).unwrap();
    let (got, total) = (AtomicU64::new(0), AtomicU64::new(0));
    let data_ok = fetch_verified(&rel, &key.embedded, &got, &total).unwrap();
    assert_eq!((got.load(Ordering::Relaxed), total.load(Ordering::Relaxed)), (new.len() as u64, new.len() as u64));
    install(&Target::Binary(exe.clone()), &data_ok).unwrap();
    assert_eq!(std::fs::read(&exe).unwrap(), new);
    assert_eq!(std::fs::read(previous_of(&exe)).unwrap(), elf("version-0.3.11"));
    let mut st = State::default();
    st.whatsnew = Some((rel.version.clone(), rel.notes.clone()));
    st.boot = Some((rel.version.clone(), 0));
    st.save(&install_dir);
    assert_eq!(State::load(&install_dir).whatsnew.unwrap().1, "Faster rooms.\nA new look.");

    // 3. refused, and the program stays as it was: wrong key, a changed file, a signature for another version, no signature
    let before = std::fs::read(&exe).unwrap();
    let wrong = TestKey::new(10);
    for (what, sig, payload) in [
        ("another key", wrong.sign(&new, "0.3.12"), new.clone()),
        ("a changed file", key.sign(&new, "0.3.12"), elf("tampered")),
        ("another version's signature", key.sign(&new, "0.3.10"), new.clone()),
    ] {
        std::fs::write(site.join("update/Megatron_0.3.12_gpui"), &payload).unwrap();
        write_feed(feed("0.3.12", &sig));
        let rel = check(&url, "0.3.11", &plat).unwrap().unwrap();
        let e = fetch_verified(&rel, &key.embedded, &AtomicU64::new(0), &AtomicU64::new(0)).unwrap_err();
        assert!(e.contains("signature") || e.contains("signed"), "{what}: {e}");
        assert_eq!(std::fs::read(&exe).unwrap(), before, "{what}: nothing was replaced");
    }
    write_feed(json!({"version": "0.3.12", "platforms": {plat.as_str(): {"url": "update/Megatron_0.3.12_gpui"}}}));
    assert!(check(&url, "0.3.11", &plat).unwrap_err().contains("signature"));

    // 4. a feed that points at another server is refused whole; a missing file is an error, not an install
    write_feed(json!({"version": "0.3.12", "platforms": {plat.as_str(): {"url": "https://evil.example.org/x", "signature": "c2ln"}}}));
    assert!(check(&url, "0.3.11", &plat).unwrap_err().contains("another server"));
    write_feed(feed("0.3.12", &key.sign(&new, "0.3.12")));
    std::fs::remove_file(site.join("update/Megatron_0.3.12_gpui")).unwrap();
    let rel = check(&url, "0.3.11", &plat).unwrap().unwrap();
    assert!(fetch_verified(&rel, &key.embedded, &AtomicU64::new(0), &AtomicU64::new(0)).is_err());
    assert_eq!(std::fs::read(&exe).unwrap(), before);
    drop(relay);
}

/// What scripts/publish-gpui.sh put in a scratch dist/ (tools/gpui-update-test.sh) is accepted by the app's own check.
#[test]
fn what_the_publish_script_made_passes_the_apps_check() {
    let (Some(dist), Some(pubf)) = (std::env::var_os("COMMONS_UPDATE_DIST"), std::env::var_os("COMMONS_UPDATE_PUBKEY_FILE")) else { eprintln!("skipped: not run by tools/gpui-update-test.sh"); return };
    let dist = PathBuf::from(dist);
    let key = std::fs::read_to_string(pubf).unwrap().trim().to_string();
    let feed: Value = serde_json::from_slice(&std::fs::read(dist.join("update/latest.json")).unwrap()).unwrap();
    let rel = release_for(&feed, "0.8.0", "linux-gpui-x86_64").unwrap().expect("the native entry");
    assert_eq!(rel.notes, "Test notes.");
    let file = std::fs::read(dist.join(&rel.url)).unwrap();
    verify(&file, &rel.signature, &key, "0.9.0").unwrap();
    assert!(verify(&file, &rel.signature, &public_key(), "0.9.0").is_err(), "the real key does not accept a throwaway-signed file");
}

#[test]
fn github_feed_files_must_be_this_apps_releases() {
    let m = |url: &str| json!({"version": "9.9.9", "platforms": {"linux-gpui-x86_64": {"url": url, "signature": "c2ln"}}});
    let ok = github_release_from(&m("https://github.com/jfj-oss/commons/releases/download/v9.9.9/mega-gpui-linux-x86_64"), "0.3.13", "linux-gpui-x86_64").unwrap().unwrap();
    assert_eq!(ok.version, "9.9.9");
    for bad in ["https://github.com/someone/else/releases/download/v9.9.9/x", "https://evil.example/x", "update/Megatron_9.9.9_gpui-x86_64",
                "https://github.com/jfj-oss/commons/releases/download/../../../someone/x"] {
        assert!(github_release_from(&m(bad), "0.3.13", "linux-gpui-x86_64").is_err(), "{bad} accepted");
    }
    assert!(github_release_from(&m("https://github.com/jfj-oss/commons/releases/download/v9.9.9/x"), "9.9.9", "linux-gpui-x86_64").unwrap().is_none(), "same version offered");
}

#[test]
fn github_redirects_only_to_github_hosts() {
    assert!(github_host_ok("https://github.com/jfj-oss/commons/releases/latest/download/mega-native.json"));
    assert!(github_host_ok("https://release-assets.githubusercontent.com/github-production-release-asset/1?sig=x"));
    assert!(github_host_ok("https://objects.githubusercontent.com/x"));
    assert!(!github_host_ok("http://github.com/x"), "plain http");
    assert!(!github_host_ok("https://github.com.evil.org/x"));
    assert!(!github_host_ok("https://evil.org/github.com/x"));
}

#[test]
fn the_newer_offer_wins_and_the_relay_on_a_tie() {
    let r = |v: &str, u: &str| Some(Release { version: v.into(), url: u.into(), signature: "s".into(), notes: String::new(), required: false });
    assert_eq!(newer(r("0.3.14", "relay"), r("0.3.15", "gh")).unwrap().url, "gh");
    assert_eq!(newer(r("0.3.15", "relay"), r("0.3.14", "gh")).unwrap().url, "relay");
    assert_eq!(newer(r("0.3.14", "relay"), r("0.3.14", "gh")).unwrap().url, "relay");
    assert_eq!(newer(None, r("0.3.14", "gh")).unwrap().url, "gh");
    assert!(newer(None, None).is_none());
}

/// Online, by hand: `cargo test -p mega-gpui github_feed_live -- --ignored`. Reads the real GitHub feed and checks the
/// file against the real key.
#[test]
#[ignore]
fn github_feed_live() {
    let rel = check_github("0.0.1", "linux-gpui-x86_64").unwrap().expect("a release");
    let (got, total) = (AtomicU64::new(0), AtomicU64::new(0));
    let data = fetch_verified(&rel, &public_key(), &got, &total).unwrap();
    eprintln!("GitHub offers {} ({} bytes, signature good)", rel.version, data.len());
}
