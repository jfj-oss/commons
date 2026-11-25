//! The native app's updater (docs/UPDATES.md, "The native app"). No UI here: the feed check, the signature check, the
//! install and the state file. updateui.rs is the window's side of it.
//!
//! The feed is the one the Tauri apps read: `<relay>/update/latest.json`, localized and checked by
//! `commons_core::update` (the account's own relay, https or loopback, no redirects, every address inside that relay).
//! This app's entry is a platform key of its own, `linux-gpui-x86_64` or `darwin-gpui-aarch64`, which no other app looks
//! for. The file is checked against the SAME minisign public key the Tauri updater trusts (read from
//! `app/src-tauri/tauri.conf.json` at build time, not copied), and for the exact version the manifest announces.
//! Nothing is replaced, and nothing is run, before that check has passed.
use commons_core::browser_update::verify;
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// A native build is tens of MB; anything near this is not one.
pub const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_UNPACKED_BYTES: u64 = 1024 * 1024 * 1024;
/// How often the feed is read: on start (a little after unlocking) and then every half hour (a tiny file), so a new
/// version reaches an app that stays open soon after it is published.
pub const CHECK_EVERY: Duration = Duration::from_secs(30 * 60);
/// "Later" waits this long before offering again.
pub const LATER: Duration = Duration::from_secs(4 * 3600);
/// A new version that does not stay up this long (counted from its start) was not a good start.
pub const BOOT_CONFIRM: Duration = Duration::from_secs(20);
/// This many starts of a new version without one lasting [`BOOT_CONFIRM`]: the previous one is put back.
pub const BOOT_TRIES: u32 = 3;

/// The version this build is. A release build is made with `MEGA_GPUI_VERSION` set (scripts/publish-gpui.sh); without it
/// the crate's own version stands (a development build, which then offers itself every update).
pub fn current_version() -> String {
    option_env!("MEGA_GPUI_VERSION").filter(|v| !v.is_empty()).unwrap_or(env!("CARGO_PKG_VERSION")).to_string()
}

/// The updater public key this build trusts: the Tauri updater's (`plugins.updater.pubkey`), so one key signs every app.
/// `MEGA_GPUI_UPDATE_PUBKEY` at BUILD time replaces it, for the tests' throwaway key; it is not read at run time.
pub fn public_key() -> String {
    if let Some(k) = option_env!("MEGA_GPUI_UPDATE_PUBKEY").filter(|k| !k.is_empty()) { return k.to_string(); }
    let conf: Value = serde_json::from_str(include_str!("../../src-tauri/tauri.conf.json")).unwrap_or(Value::Null);
    conf["plugins"]["updater"]["pubkey"].as_str().unwrap_or("").to_string()
}

/// The feed key for a system. Not a Tauri updater target (`linux-x86_64`, `darwin-aarch64`, ...), nor a browser one
/// (`linux-browser-x86_64`), so no other app takes this entry for its own. Windows has no native build.
pub fn platform_key(os: &str, arch: &str) -> Option<String> {
    let os = match os { "linux" => "linux", "macos" => "darwin", _ => return None };
    let arch = match arch { "x86_64" | "aarch64" => arch, _ => return None };
    Some(format!("{os}-gpui-{arch}"))
}
pub fn platform() -> Option<String> { platform_key(std::env::consts::OS, std::env::consts::ARCH) }

/// Where an update is, for the window.
#[derive(Debug, Clone, PartialEq)]
pub enum Phase { Idle, Checking, Available(Release), Downloading(Release), Ready(String), Failed(String) }

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub version: String,
    pub url: String,
    /// The manifest's base64 of the minisign `.sig` text, as the Tauri updater takes it.
    pub signature: String,
    /// The release notes: the text publish-update.sh puts in the feed.
    pub notes: String,
    /// The relay says this version is too old to go on without updating.
    pub required: bool,
}

/// The newer release the manifest announces for `platform`. `Ok(None)`: nothing newer, or no native build of that
/// version is published yet (the feed lists the other apps first); an older or equal version is never offered.
pub fn release_for(manifest: &Value, current: &str, platform: &str) -> Result<Option<Release>, String> {
    let Some(version) = manifest.get("version").and_then(Value::as_str) else { return Ok(None) };
    if commons_core::update::parse_version(version).is_none() { return Ok(None); }
    if !commons_core::update::version_lt(current, version).unwrap_or(false) { return Ok(None); }
    let Some(p) = manifest.get("platforms").and_then(|p| p.get(platform)) else { return Ok(None) };
    let url = p.get("url").and_then(Value::as_str).filter(|u| !u.is_empty()).ok_or("the update feed has no file address for this app")?;
    let signature = p.get("signature").and_then(Value::as_str).filter(|s| !s.trim().is_empty()).ok_or("this update has no signature; refused")?;
    Ok(Some(Release {
        version: version.to_string(), url: url.to_string(), signature: signature.to_string(),
        notes: manifest.get("notes").and_then(Value::as_str).unwrap_or("").trim().to_string(),
        required: commons_core::update::below_minimum(current, manifest),
    }))
}

/// The public feed: the newest GitHub release of the app's repository carries `mega-native.json` (same shape as the
/// relay's manifest, file addresses on GitHub). It reaches friends whose relay is off or far away. The signature check
/// is the same, so GitHub (or anyone between) can at worst withhold an update, never change one.
pub const GITHUB_FEED: &str = "https://github.com/jfj-oss/commons/releases/latest/download/mega-native.json";
/// Release files must be in this repository's releases ...
pub const GITHUB_FILES: &str = "https://github.com/jfj-oss/commons/releases/download/";
/// ... and GitHub may only send the download on to its own file hosts.
const GITHUB_HOSTS: [&str; 3] = ["github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com"];
const MAX_MANIFEST_BYTES: u64 = 256 * 1024;

pub(crate) fn github_host_ok(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    GITHUB_HOSTS.contains(&host)
}

/// GET with redirects followed by hand: at most five, each one https to a GitHub host.
fn github_get(url: &str, accept: &str, read_timeout: Duration) -> Result<ureq::Response, String> {
    let agent = ureq::AgentBuilder::new().redirects(0).timeout_connect(Duration::from_secs(8)).timeout_read(Duration::from_secs(30)).timeout(read_timeout).build();
    let mut at = url.to_string();
    for _ in 0..6 {
        if !github_host_ok(&at) { return Err("the update address is not on GitHub; refused".into()); }
        let r = match agent.get(&at).set("accept", accept).set("user-agent", "mega-updater").call() {
            Ok(r) => r,
            Err(ureq::Error::Status(code, _)) => return Err(format!("GitHub answered {code}")),
            Err(e) => return Err(format!("could not reach GitHub: {e}")),
        };
        match r.status() {
            200 => return Ok(r),
            301 | 302 | 303 | 307 | 308 => { at = r.header("location").ok_or("a GitHub redirect without an address")?.to_string(); }
            c => return Err(format!("GitHub answered {c}")),
        }
    }
    Err("too many GitHub redirects".into())
}

/// The GitHub feed's offer. `Ok(None)` when there is no release, or nothing newer.
pub fn check_github(current: &str, platform: &str) -> Result<Option<Release>, String> {
    let r = match github_get(GITHUB_FEED, "application/json, application/octet-stream", Duration::from_secs(20)) {
        Err(e) if e.contains("answered 404") => return Ok(None),
        x => x?,
    };
    let mut body = Vec::new();
    r.into_reader().take(MAX_MANIFEST_BYTES + 1).read_to_end(&mut body).map_err(|e| format!("could not read the GitHub update feed: {e}"))?;
    if body.len() as u64 > MAX_MANIFEST_BYTES { return Err("the GitHub update feed is too large; refused".into()); }
    let m: Value = serde_json::from_slice(&body).map_err(|e| format!("the GitHub update feed is not valid JSON: {e}"))?;
    github_release_from(&m, current, platform)
}

/// The GitHub feed's manifest read as [`release_for`] does, but its file must be one of the app's own releases.
pub fn github_release_from(m: &Value, current: &str, platform: &str) -> Result<Option<Release>, String> {
    let rel = release_for(m, current, platform)?;
    if let Some(r) = &rel { if !r.url.starts_with(GITHUB_FILES) || r.url.contains("..") { return Err("the GitHub update feed points outside the app's releases; refused".into()); } }
    Ok(rel)
}

/// GitHub is asked as well unless `MEGA_GPUI_GITHUB=off`, or the relay is this computer (the tests' relays are).
fn github_wanted(relay: &str) -> bool {
    if std::env::var("MEGA_GPUI_GITHUB").is_ok_and(|v| v == "off") { return false; }
    !(relay.starts_with("http://127.0.0.1") || relay.starts_with("http://localhost"))
}

/// Of two offers, the newer one (the relay's when they are the same version: it is nearer).
pub(crate) fn newer(a: Option<Release>, b: Option<Release>) -> Option<Release> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if commons_core::update::version_lt(&a.version, &b.version).unwrap_or(false) { b } else { a }),
        (a, b) => a.or(b),
    }
}

/// Reads the relay's feed and GitHub's and says what they offer this app. Either one answering is enough; an error
/// only comes back when both fail. Network: run it off the window's thread.
pub fn check(relay: &str, current: &str, platform: &str) -> Result<Option<Release>, String> {
    let from_relay = match commons_core::update::fetch_manifest(relay) {
        Ok(Some(m)) => release_for(&m, current, platform),
        Ok(None) => Ok(None),
        Err(e) => Err(e),
    };
    if !github_wanted(relay) { return from_relay; }
    let from_github = check_github(current, platform);
    match (from_relay, from_github) {
        (Ok(a), Ok(b)) => Ok(newer(a, b)),
        (Ok(a), Err(e)) => { eprintln!("update check (GitHub): {e}"); Ok(a) }
        (Err(e), Ok(b)) => { eprintln!("update check (relay): {e}"); Ok(b) }
        (Err(e), Err(g)) => Err(format!("{e}; GitHub: {g}")),
    }
}

/// Downloads a file the feed localized onto the relay: no redirects, a size cap before and while reading. `got` counts
/// the bytes so far, for the progress line.
pub fn download(url: &str, got: &AtomicU64, total: &AtomicU64) -> Result<Vec<u8>, String> {
    let resp = if url.starts_with(GITHUB_FILES) {
        github_get(url, "application/octet-stream", Duration::from_secs(900)).map_err(|e| format!("the update could not be downloaded: {e}"))?
    } else {
        let agent = ureq::AgentBuilder::new().redirects(0).timeout_connect(Duration::from_secs(8)).timeout_read(Duration::from_secs(30)).timeout(Duration::from_secs(900)).build();
        match agent.get(url).set("accept", "application/octet-stream").call() {
            Ok(r) if r.status() == 200 => r,
            Ok(r) => return Err(format!("the update download answered {}; refused", r.status())),
            Err(ureq::Error::Status(code, _)) => return Err(format!("the update download answered {code}; refused")),
            Err(e) => return Err(format!("the update could not be downloaded: {e}")),
        }
    };
    let len = resp.header("content-length").and_then(|v| v.trim().parse::<u64>().ok());
    if len.is_some_and(|l| l > MAX_DOWNLOAD_BYTES) { return Err("the update file is too large; refused".into()); }
    total.store(len.unwrap_or(0), Ordering::Relaxed);
    let mut r = resp.into_reader().take(MAX_DOWNLOAD_BYTES + 1);
    let mut body = Vec::with_capacity(len.unwrap_or(1 << 20) as usize);
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = r.read(&mut buf).map_err(|e| format!("the update could not be downloaded: {e}"))?;
        if n == 0 { break; }
        body.extend_from_slice(&buf[..n]);
        got.store(body.len() as u64, Ordering::Relaxed);
    }
    if body.len() as u64 > MAX_DOWNLOAD_BYTES { return Err("the update file is too large; refused".into()); }
    Ok(body)
}

/// Download and check. What comes back has passed the signature check for exactly this release's version.
pub fn fetch_verified(rel: &Release, public_key_b64: &str, got: &AtomicU64, total: &AtomicU64) -> Result<Vec<u8>, String> {
    let data = download(&rel.url, got, total)?;
    verify(&data, &rel.signature, public_key_b64, &rel.version)?;
    Ok(data)
}

// ---- installing ----

/// Where this app is installed, and how it is replaced.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// Linux: the executable file itself.
    Binary(PathBuf),
    /// macOS: the `.app` bundle the executable is inside.
    Bundle(PathBuf),
}

/// The install this process runs from, taken at start (after a replace, `current_exe` reads "... (deleted)").
pub fn target_of(exe: &Path) -> Result<Target, String> {
    if cfg!(target_os = "macos") {
        let app = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).ok_or("mega is not inside an app bundle, so it cannot update itself")?;
        Ok(Target::Bundle(app.to_path_buf()))
    } else if cfg!(target_os = "linux") {
        Ok(Target::Binary(exe.to_path_buf()))
    } else { Err("this system has no native mega build yet".into()) }
}

fn side(path: &Path, tag: &str) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "mega".into());
    path.with_file_name(format!(".{name}.{tag}"))
}
/// The copy kept for going back: `<name>.previous` next to the file (`Megatron.previous.app` for a bundle).
pub fn previous_of(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "mega".into());
    match name.strip_suffix(".app") { Some(stem) => path.with_file_name(format!("{stem}.previous.app")), None => path.with_file_name(format!("{name}.previous")) }
}
fn why(what: &str, p: &Path, e: std::io::Error) -> String {
    let hint = if e.kind() == std::io::ErrorKind::PermissionDenied { " (the folder is not writable: put mega somewhere you can write to)" } else { "" };
    format!("{what} {}: {e}{hint}", p.display())
}

/// Replaces the executable with `new`, atomically: the new file is written next to it and synced, the old one is kept
/// as `<name>.previous` (a hard link, so the running file is untouched), and one rename puts the new one in its place.
/// A running process keeps its old file open, so this is safe while the app runs. Every failure leaves the old
/// executable where it was.
pub fn install_binary(exe: &Path, new: &[u8]) -> Result<(), String> {
    if cfg!(target_os = "linux") && !new.starts_with(b"\x7fELF") { return Err("the update is not a Linux program; refused".into()); }
    if new.len() < 1024 { return Err("the update file is too small to be the app; refused".into()); }
    let tmp = side(exe, "update-new");
    let result = (|| {
        {
            use std::io::Write;
            let _ = std::fs::remove_file(&tmp);
            let mut f = std::fs::File::create(&tmp).map_err(|e| why("could not write the update next to", exe, e))?;
            f.write_all(new).map_err(|e| why("could not write the update next to", exe, e))?;
            f.sync_all().map_err(|e| why("could not write the update next to", exe, e))?;
        }
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).map_err(|e| why("could not mark the update runnable", &tmp, e))?; }
        // keep the one we have: a hard link under a temporary name, renamed over any older copy
        let keep = side(exe, "prev-tmp");
        let _ = std::fs::remove_file(&keep);
        if exe.exists() {
            if std::fs::hard_link(exe, &keep).is_err() { std::fs::copy(exe, &keep).map_err(|e| why("could not keep the previous version of", exe, e))?; }
            std::fs::rename(&keep, previous_of(exe)).map_err(|e| why("could not keep the previous version of", exe, e))?;
        }
        std::fs::rename(&tmp, exe).map_err(|e| why("could not put the update in place of", exe, e))
    })();
    if result.is_err() { let _ = std::fs::remove_file(&tmp); }
    result
}

/// Puts the previous executable back (one rename over the current one).
pub fn rollback_binary(exe: &Path) -> Result<(), String> {
    let prev = previous_of(exe);
    if !prev.exists() { return Err("there is no previous version to go back to".into()); }
    std::fs::rename(&prev, exe).map_err(|e| why("could not put the previous version back at", exe, e))
}

/// A path inside a bundle archive: plain names under one top-level `<name>.app`; no `..`, no root, nothing that climbs out.
fn safe_entry(path: &Path) -> Result<(), String> {
    let mut comps = path.components();
    match comps.next() { Some(Component::Normal(c)) if c.to_string_lossy().ends_with(".app") => {} _ => return Err(format!("the update archive has an unexpected entry ({})", path.display())) }
    if comps.any(|c| !matches!(c, Component::Normal(_))) { return Err(format!("the update archive has an unsafe path ({})", path.display())); }
    Ok(())
}
/// Unpacks a `.app.tar.gz` into `dest` and returns the bundle inside. Files, folders and relative links that stay
/// inside the bundle; nothing else is written.
pub fn unpack_bundle(tar_gz: &[u8], dest: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dest).map_err(|e| why("could not make a folder for the update at", dest, e))?;
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(tar_gz));
    let (mut total, mut top) = (0u64, None::<PathBuf>);
    for entry in ar.entries().map_err(|e| format!("the update archive is damaged: {e}"))? {
        let mut entry = entry.map_err(|e| format!("the update archive is damaged: {e}"))?;
        let path = entry.path().map_err(|e| format!("the update archive is damaged: {e}"))?.into_owned();
        safe_entry(&path)?;
        let first = PathBuf::from(path.components().next().map(|c| c.as_os_str().to_owned()).unwrap_or_default());
        match &top { None => top = Some(first), Some(t) if *t != first => return Err("the update archive holds more than one app".into()), _ => {} }
        total += entry.size();
        if total > MAX_UNPACKED_BYTES { return Err("the update archive unpacks to too much; refused".into()); }
        let out = dest.join(&path);
        let kind = entry.header().entry_type();
        if kind.is_dir() { std::fs::create_dir_all(&out).map_err(|e| why("could not unpack", &out, e))?; }
        else if kind.is_file() {
            if let Some(d) = out.parent() { std::fs::create_dir_all(d).map_err(|e| why("could not unpack", d, e))?; }
            entry.unpack(&out).map_err(|e| why("could not unpack", &out, e))?;
        } else if kind.is_symlink() {
            let link = entry.link_name().map_err(|e| format!("the update archive is damaged: {e}"))?.ok_or("a link in the update archive has no target")?.into_owned();
            // inside the bundle only: relative, and never above the bundle's own folder
            let mut depth = path.components().count() as i64 - 2;
            for c in link.components() { match c { Component::Normal(_) => depth += 1, Component::ParentDir => depth -= 1, _ => return Err("a link in the update archive points outside the app; refused".into()) } if depth < 0 { return Err("a link in the update archive points outside the app; refused".into()); } }
            if let Some(d) = out.parent() { std::fs::create_dir_all(d).map_err(|e| why("could not unpack", d, e))?; }
            #[cfg(unix)] std::os::unix::fs::symlink(&link, &out).map_err(|e| why("could not unpack", &out, e))?;
        } else { return Err(format!("the update archive has an entry that is not a file or folder ({})", path.display())); }
    }
    let top = top.ok_or("the update archive is empty")?;
    Ok(dest.join(top))
}

/// macOS: the replaced bundle is signed ad hoc again (the archive's own signature does not survive a build on another
/// computer) and the result is checked, before it goes anywhere near the old one.
#[cfg(target_os = "macos")]
fn seal(app: &Path) -> Result<(), String> {
    let run = |args: &[&str]| std::process::Command::new("/usr/bin/codesign").args(args).arg(app).output().map_err(|e| format!("could not run codesign: {e}"));
    let o = run(&["--force", "--deep", "--sign", "-"])?;
    if !o.status.success() { return Err(format!("could not sign the update: {}", String::from_utf8_lossy(&o.stderr).trim())); }
    let o = run(&["--verify", "--deep"])?;
    if !o.status.success() { return Err(format!("the signed update did not verify: {}", String::from_utf8_lossy(&o.stderr).trim())); }
    Ok(())
}
#[cfg(not(target_os = "macos"))]
fn seal(_app: &Path) -> Result<(), String> { Ok(()) }

/// macOS: replaces the `.app` bundle. The new one is unpacked next to it, sealed, then two renames swap them (the old
/// one is kept as `<name>.previous.app`); if the second fails the first is undone.
pub fn install_bundle(app: &Path, tar_gz: &[u8]) -> Result<(), String> {
    let stage = side(app, "update-stage");
    let _ = std::fs::remove_dir_all(&stage);
    let result = (|| {
        let new = unpack_bundle(tar_gz, &stage)?;
        seal(&new)?;
        let prev = previous_of(app);
        let _ = std::fs::remove_dir_all(&prev);
        if app.exists() { std::fs::rename(app, &prev).map_err(|e| why("could not set aside", app, e))?; }
        if let Err(e) = std::fs::rename(&new, app) {
            if prev.exists() { let _ = std::fs::rename(&prev, app); }
            return Err(why("could not put the update in place of", app, e));
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&stage);
    result
}
pub fn rollback_bundle(app: &Path) -> Result<(), String> {
    let prev = previous_of(app);
    if !prev.exists() { return Err("there is no previous version to go back to".into()); }
    let gone = side(app, "bad");
    let _ = std::fs::remove_dir_all(&gone);
    if app.exists() { std::fs::rename(app, &gone).map_err(|e| why("could not set aside", app, e))?; }
    std::fs::rename(&prev, app).map_err(|e| why("could not put the previous version back at", app, e))?;
    let _ = std::fs::remove_dir_all(&gone);
    Ok(())
}

pub fn install(target: &Target, data: &[u8]) -> Result<(), String> {
    match target { Target::Binary(p) => install_binary(p, data), Target::Bundle(p) => install_bundle(p, data) }
}
pub fn rollback(target: &Target) -> Result<(), String> {
    match target { Target::Binary(p) => rollback_binary(p), Target::Bundle(p) => rollback_bundle(p) }
}

// ---- the state file ----

/// What the app remembers about updates, beside its other window preferences (`gpui-update.json`, never inside the
/// core's own files).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    /// The version just installed, and the notes it came with: shown once, on its first launch.
    pub whatsnew: Option<(String, String)>,
    /// A new version's starts so far without one lasting [`BOOT_CONFIRM`].
    pub boot: Option<(String, u32)>,
    /// A version that failed to start and was rolled back from: said once.
    pub rolled_back: Option<String>,
}
fn state_path(dir: &Path) -> PathBuf { dir.join("gpui-update.json") }
impl State {
    pub fn load(dir: &Path) -> State {
        let v: Value = std::fs::read(state_path(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
        let s = |x: &Value| x.as_str().map(String::from);
        State {
            whatsnew: s(&v["whatsnew"]["version"]).map(|ver| (ver, s(&v["whatsnew"]["notes"]).unwrap_or_default())),
            boot: s(&v["boot"]["version"]).map(|ver| (ver, v["boot"]["tries"].as_u64().unwrap_or(0) as u32)),
            rolled_back: s(&v["rolled_back"]),
        }
    }
    pub fn save(&self, dir: &Path) {
        let v = json!({
            "whatsnew": self.whatsnew.as_ref().map(|(v, n)| json!({"version": v, "notes": n})),
            "boot": self.boot.as_ref().map(|(v, t)| json!({"version": v, "tries": t})),
            "rolled_back": self.rolled_back,
        });
        if dir.is_dir() { let tmp = dir.join("gpui-update.json.tmp"); if std::fs::write(&tmp, serde_json::to_vec_pretty(&v).unwrap_or_default()).is_ok() { let _ = std::fs::rename(&tmp, state_path(dir)); } }
    }
}

/// What start-up does about a version that is not yet known to start well.
#[derive(Debug, PartialEq)]
pub enum Boot { Fine, Counted(u32), RollBack(String) }
/// `current` is starting. Pure: the caller saves the state and acts.
pub fn boot_step(state: &mut State, current: &str) -> Boot {
    let Some((v, tries)) = state.boot.clone() else { return Boot::Fine };
    if v != current { state.boot = None; return Boot::Fine; }
    if tries + 1 >= BOOT_TRIES { state.boot = None; state.rolled_back = Some(v.clone()); state.whatsnew = None; return Boot::RollBack(v); }
    state.boot = Some((v, tries + 1));
    Boot::Counted(tries + 1)
}

/// Called first thing in `main`. A new version that has now failed to start [`BOOT_TRIES`] times in a row is replaced by
/// the previous one, which is started in its place (the process does not return). Otherwise it only counts the start.
pub fn guard_start(dir: &Path) {
    if std::env::var_os("MEGA_GPUI_UPDATES").is_some_and(|v| v == "off") { return; }
    let mut st = State::load(dir);
    if st.boot.is_none() { return; }
    let outcome = boot_step(&mut st, &current_version());
    st.save(dir);
    if let Boot::RollBack(v) = outcome {
        let Ok(exe) = std::env::current_exe() else { return };
        let Ok(target) = target_of(&exe) else { return };
        if rollback(&target).is_ok() {
            eprintln!("mega {v} did not start well {BOOT_TRIES} times; went back to the previous version");
            restart_into(&target, true);
        }
    }
}

/// The program to start for a target: the file itself, or the bundle's executable (the same name inside).
pub fn exe_in(target: &Target, old_exe: &Path) -> PathBuf {
    match target {
        Target::Binary(p) => p.clone(),
        Target::Bundle(app) => {
            // the old executable's path below the bundle, onto the (new) bundle
            old_exe.strip_prefix(app).map(|rel| app.join(rel)).unwrap_or_else(|_| old_exe.to_path_buf())
        }
    }
}
/// Starts the installed program. `replace`: this process becomes it (`exec`); otherwise it starts once this process has
/// gone, so the core's lock on the account folder is free (a shell waits for our pid, then execs).
pub fn restart_into(target: &Target, replace: bool) {
    let Ok(old) = std::env::current_exe().map(|p| PathBuf::from(p.to_string_lossy().trim_end_matches(" (deleted)"))) else { return };
    let exe = exe_in(target, &old);
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        if replace { let e = std::process::Command::new(&exe).args(&args).exec(); eprintln!("could not restart: {e}"); return; }
        let _ = std::process::Command::new("/bin/sh")
            .arg("-c").arg("while kill -0 \"$0\" 2>/dev/null; do sleep 0.2; done; exec \"$@\"").arg(std::process::id().to_string()).arg(&exe).args(&args)
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).process_group(0).spawn();
    }
    #[cfg(not(unix))] { let _ = (exe, args, replace); }
}

#[cfg(test)]
mod tests;
