//! Signed updates for the "browser versions" (packaging/linux-browser, -windows-browser, -macos-browser,
//! docs/UPDATES.md).
//!
//! The launcher runs `bridge --update <relay> <install-dir>` before it starts: the bridge fetches the same
//! `<relay>/update/latest.json` the desktop app uses, takes the entry for this system (`linux-browser-x86_64`,
//! `windows-browser-x86_64`, or `darwin-browser-aarch64`; no desktop Tauri target matches any of them, so desktop apps
//! never see it), downloads the tarball, and checks its minisign signature against the public key embedded in this
//! build. Nothing is installed until the signature, the version in it, and the "newer than this build" rule all pass.
//! The tarball is unpacked into a temporary directory next to the install and its `ui/`, `bridge` (and `launcher`,
//! where the package has one) are then renamed into place; the previous copy stays in `.update-previous` until the new
//! bridge has started (it removes the old copy itself after it binds its port), so a failed start can be rolled back.
//!
//! On Windows the file being replaced is `bridge.exe`. The update runs in a child process (`bridge.exe --update`), so
//! the file it replaces is its own running image: it is renamed aside (Windows allows renaming a running executable)
//! and the new one is moved into place, rather than being overwritten.
//!
//! The manifest is fetched and localized by [`crate::update`], so the same rules apply as for the desktop app: only the
//! account's relay, no redirects, https (plain http only on this computer). Downloads never follow redirects either.
//! A small note is left in the install directory for the page to show: "Updated to x.y.z" after an update, or
//! "Update available" and the reason when a newer version exists but could not be installed.

use flate2::read::GzDecoder;
use minisign_verify::{PublicKey, Signature};
use serde_json::Value;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

/// The manifest key for this system's browser version. It is deliberately not a Tauri updater target (`linux-x86_64`,
/// `windows-x86_64`, `darwin-aarch64`, ...), so desktop apps ignore it. The macOS build is Apple silicon; an Intel Mac
/// build would need its own key (`darwin-browser-x86_64`) to be published separately.
#[cfg(target_os = "windows")]
pub const PLATFORM: &str = "windows-browser-x86_64";
#[cfg(target_os = "macos")]
pub const PLATFORM: &str = "darwin-browser-aarch64";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub const PLATFORM: &str = "linux-browser-x86_64";

/// The bridge executable's name in an install and in an update tarball.
pub fn bridge_name() -> &'static str {
    if cfg!(windows) { "bridge.exe" } else { "bridge" }
}

/// The launcher's name in an update tarball. Windows and macOS packages carry one; the Linux shell launcher is not in
/// the tarball, so this is only consulted for an update that actually contains a launcher.
pub fn launcher_name() -> &'static str {
    if cfg!(windows) { "launcher.exe" } else { "launcher" }
}

/// A published Linux browser tarball is a few tens of MB; anything near this is not one.
pub const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;
/// The unpacked files are at most this much (the tarball is compressed, so this is the real ceiling).
pub const MAX_UNPACKED_BYTES: u64 = 1024 * 1024 * 1024;

/// The version this build was made for, embedded by scripts/build-linux-browser.sh.
pub fn version() -> &'static str {
    option_env!("MEGATRON_BROWSER_VERSION").filter(|v| !v.is_empty()).unwrap_or(env!("CARGO_PKG_VERSION"))
}

/// The updater public key this build trusts (the same as the desktop app's `plugins.updater.pubkey`, base64 of the
/// minisign `.pub` text), embedded by scripts/build-linux-browser.sh. A build without it refuses every update.
pub fn public_key() -> Option<&'static str> {
    option_env!("MEGATRON_BROWSER_PUBKEY").filter(|v| !v.is_empty())
}

#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    pub url: String,
    /// The manifest's base64 of the minisign `.sig` text, exactly as the Tauri updater takes it.
    pub signature: String,
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// This relay publishes nothing newer (or the manifest is for a version that is not newer: no downgrades).
    None,
    Updated { version: String },
    Available { version: String, reason: String },
}

/// The newer release a manifest announces for this system's browser version. `Ok(None)` means "nothing newer": an
/// older or equal version is refused silently, as a downgrade (the person is not told about something that will never
/// be installed).
pub fn release(manifest: &Value, current: &str) -> Result<Option<Release>, String> {
    release_for(manifest, current, PLATFORM)
}

/// [`release`] for one named platform; the tests use it for all three.
pub fn release_for(manifest: &Value, current: &str, platform: &str) -> Result<Option<Release>, String> {
    let version = match manifest.get("version").and_then(Value::as_str) {
        Some(v) => v.to_string(),
        None => return Ok(None),
    };
    if crate::update::parse_version(&version).is_none() {
        return Ok(None);
    }
    if !crate::update::version_lt(current, &version).unwrap_or(false) {
        return Ok(None);
    }
    let p = manifest
        .get("platforms")
        .and_then(|p| p.get(platform))
        .ok_or_else(|| "no browser version is published for this system yet".to_string())?;
    let url = p
        .get("url")
        .and_then(Value::as_str)
        .filter(|u| !u.is_empty())
        .ok_or_else(|| "the update manifest has no file address for the browser version".to_string())?;
    let signature = p
        .get("signature")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "the browser update has no signature; refused".to_string())?;
    Ok(Some(Release { version, url: url.to_string(), signature: signature.to_string() }))
}

fn b64_text(s: &str) -> Result<String, String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .map_err(|e| format!("the update signature is not valid base64: {e}"))?;
    String::from_utf8(bytes).map_err(|_| "the update signature is not text".to_string())
}

/// Checks a downloaded tarball against the embedded minisign key. The signature must be made for exactly `version`
/// (the version in the signature's trusted comment, the same rule the desktop updater uses), and it must be a
/// pre-hashed signature as the Tauri signer produces: an unsigned or legacy file is refused.
pub fn verify(data: &[u8], signature_b64: &str, public_key_b64: &str, version: &str) -> Result<(), String> {
    if public_key_b64.trim().is_empty() {
        return Err("this build has no update key, so it cannot check updates; refused".into());
    }
    if signature_b64.trim().is_empty() {
        return Err("the update has no signature; refused".into());
    }
    let sig_text = b64_text(signature_b64)?;
    let sig = Signature::decode(&sig_text).map_err(|e| format!("the update signature is malformed: {e}"))?;
    let pub_text = b64_text(public_key_b64)?;
    let key = PublicKey::decode(&pub_text).map_err(|e| format!("the embedded update key is malformed: {e}"))?;
    key.verify(data, &sig, false).map_err(|e| format!("the update did not pass its signature check: {e}"))?;
    // Only now is the trusted comment trustworthy: the check above is what covers it.
    let signed = sig
        .trusted_comment()
        .split('\t')
        .find_map(|f| f.strip_prefix("version:"))
        .ok_or_else(|| "the update is not signed for a version; refused".to_string())?;
    let same = match (crate::update::parse_version(signed), crate::update::parse_version(version)) {
        (Some(a), Some(b)) => a == b,
        _ => signed.trim_start_matches('v') == version.trim_start_matches('v'),
    };
    if !same {
        return Err(format!("the update file is signed for version {signed}, not {version}; refused"));
    }
    Ok(())
}

/// Downloads a file the manifest localized onto the relay. Redirects are not followed and the size is capped before
/// (Content-Length) and while reading.
fn download(url: &str) -> Result<Vec<u8>, String> {
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(Duration::from_secs(5))
        .timeout(Duration::from_secs(120))
        .build();
    let resp = match agent.get(url).set("accept", "application/octet-stream").call() {
        Ok(r) => r,
        Err(ureq::Error::Status(code, _)) => return Err(format!("the update download answered {code}; refused")),
        Err(e) => return Err(format!("the update could not be downloaded: {e}")),
    };
    if resp.status() != 200 {
        return Err(format!("the update download answered {}; refused", resp.status()));
    }
    if let Some(len) = resp.header("content-length").and_then(|v| v.trim().parse::<u64>().ok()) {
        if len > MAX_DOWNLOAD_BYTES {
            return Err(format!("the update file is too large ({len} bytes); refused"));
        }
    }
    let mut body = Vec::new();
    resp.into_reader()
        .take(MAX_DOWNLOAD_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|e| format!("the update could not be downloaded: {e}"))?;
    if body.len() as u64 > MAX_DOWNLOAD_BYTES {
        return Err("the update file is too large; refused".into());
    }
    Ok(body)
}

/// A path inside the update tarball, relative to the temporary directory. All entries must live under one top-level
/// `megatron-browser/` directory; absolute paths, `..`, and anything else that could land outside are refused.
fn safe_rel(path: &Path) -> Result<PathBuf, String> {
    let comps: Vec<Component> = path.components().collect();
    match comps.first() {
        Some(Component::Normal(c)) if *c == "megatron-browser" => {}
        _ => return Err(format!("the update tarball has an unexpected top-level entry ({})", path.display())),
    }
    if comps.iter().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(format!("the update tarball has an unsafe path ({})", path.display()));
    }
    Ok(comps.iter().collect())
}

/// Unpacks a signed tarball into `dest`, which must exist and be empty. Only plain files and directories are accepted:
/// symlinks, hard links and device entries are refused. Returns the `megatron-browser` directory it extracted.
pub fn unpack(bytes: &[u8], dest: &Path) -> Result<PathBuf, String> {
    let mut archive = tar::Archive::new(GzDecoder::new(bytes));
    let mut total = 0u64;
    for entry in archive.entries().map_err(|e| format!("the update file is not a tarball: {e}"))? {
        let mut entry = entry.map_err(|e| format!("the update tarball could not be read: {e}"))?;
        let path = entry.path().map_err(|e| format!("the update tarball has an unusable path: {e}"))?.into_owned();
        let rel = safe_rel(&path)?;
        match entry.header().entry_type() {
            tar::EntryType::Directory => {
                std::fs::create_dir_all(dest.join(&rel)).map_err(|e| format!("could not unpack the update: {e}"))?;
            }
            tar::EntryType::Regular => {
                if rel.as_os_str().is_empty() {
                    return Err("the update tarball has a file where a folder belongs; refused".into());
                }
                total += entry.header().size().unwrap_or(0);
                if total > MAX_UNPACKED_BYTES {
                    return Err("the unpacked update is too large; refused".into());
                }
                let out = dest.join(&rel);
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| format!("could not unpack the update: {e}"))?;
                }
                let mut f = std::fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&out)
                    .map_err(|e| format!("could not unpack the update: {e}"))?;
                std::io::copy(&mut entry, &mut f).map_err(|e| format!("could not unpack the update: {e}"))?;
                f.flush().ok();
                let executable = entry.header().mode().unwrap_or(0o644) & 0o111 != 0;
                set_mode(&out, if executable { 0o755 } else { 0o644 })?;
            }
            other => {
                return Err(format!("the update tarball contains an unsupported entry ({other:?}); refused"));
            }
        }
    }
    let root = dest.join("megatron-browser");
    if !root.join(bridge_name()).is_file() || !root.join("ui").is_dir() {
        return Err(format!("the update tarball does not contain a browser version ({} and ui/); refused", bridge_name()));
    }
    Ok(root)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|e| format!("could not set permissions on the unpacked update: {e}"))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), String> {
    Ok(())
}

/// A temporary directory that is removed when it goes out of scope, including on every failure path.
struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A new, empty directory next to the install (same filesystem, so the renames in [`swap`] are atomic).
fn temp_next_to(install: &Path) -> Result<TempDir, String> {
    let parent = install.parent().ok_or("the install folder has no parent; cannot update")?;
    let name = install.file_name().ok_or("the install folder has no name; cannot update")?.to_string_lossy();
    for n in 0..20u32 {
        let p = parent.join(format!(".{name}-update-{}-{n}", std::process::id()));
        match std::fs::create_dir(&p) {
            Ok(()) => return Ok(TempDir(p)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("could not make a temporary folder next to the install: {e}")),
        }
    }
    Err("could not make a temporary folder next to the install".into())
}

/// One file or directory an update replaces: `ui/`, the bridge, and the launcher when the package carries one.
struct Item {
    name: &'static str,
    dir: bool,
}

fn remove_path(p: &Path, dir: bool) {
    if dir {
        let _ = std::fs::remove_dir_all(p);
    } else {
        let _ = std::fs::remove_file(p);
    }
}

/// Renames the new `ui/`, bridge (and launcher, when the package has one) into place, keeping the previous ones in
/// `<install>/.update-previous`. On any failure everything is put back; the returned error says what went wrong.
pub fn swap(install: &Path, new_root: &Path) -> Result<PathBuf, String> {
    swap_inner(install, new_root, None)
}

/// `fail_after`: test hook. Stop after this many successful renames and roll back, as if the next one failed.
fn swap_inner(install: &Path, new_root: &Path, fail_after: Option<usize>) -> Result<PathBuf, String> {
    let meta = |p: &Path| std::fs::symlink_metadata(p).ok().map(|m| m.file_type());
    let mut items = vec![Item { name: "ui", dir: true }, Item { name: bridge_name(), dir: false }];
    // Windows and macOS packages also update the launcher. A running launcher can be renamed aside on both systems
    // (Windows allows renaming a running executable), so it is part of the same transaction.
    if meta(&new_root.join(launcher_name())).is_some_and(|t| t.is_file()) {
        items.push(Item { name: launcher_name(), dir: false });
    }
    for it in &items {
        let old_ok = match meta(&install.join(it.name)) {
            Some(t) => if it.dir { t.is_dir() } else { t.is_file() },
            None => false,
        };
        let new_ok = match meta(&new_root.join(it.name)) {
            Some(t) => if it.dir { t.is_dir() } else { t.is_file() },
            None => false,
        };
        if !old_ok || !new_ok {
            return Err(format!("the update is not a complete browser version ({} is missing); refused", it.name));
        }
    }
    let backup = install.join(".update-previous");
    if backup.exists() {
        std::fs::remove_dir_all(&backup).map_err(|e| format!("could not remove the previous update copy: {e}"))?;
    }
    std::fs::create_dir(&backup).map_err(|e| format!("could not keep a copy of the previous version: {e}"))?;

    let mut moved_old = vec![false; items.len()];
    let mut put_new = vec![false; items.len()];
    let mut step = 0usize;
    let do_step = |n: usize| -> Result<(), String> {
        if fail_after.is_some_and(|f| f < n) {
            return Err("test: a rename failed".into());
        }
        Ok(())
    };
    let result = (|| -> Result<(), String> {
        for (i, it) in items.iter().enumerate() {
            step += 1;
            do_step(step)?;
            std::fs::rename(install.join(it.name), backup.join(it.name))
                .map_err(|e| format!("could not move the old {} aside: {e}", it.name))?;
            moved_old[i] = true;
            step += 1;
            do_step(step)?;
            std::fs::rename(new_root.join(it.name), install.join(it.name))
                .map_err(|e| format!("could not put the new {} in place: {e}", it.name))?;
            put_new[i] = true;
        }
        Ok(())
    })();
    if let Err(e) = result {
        // Put back what was moved, newest first. Every step is best-effort: report the original error.
        for (i, it) in items.iter().enumerate().rev() {
            if put_new[i] {
                remove_path(&install.join(it.name), it.dir);
            }
            if moved_old[i] {
                let _ = std::fs::rename(backup.join(it.name), install.join(it.name));
            }
        }
        let _ = std::fs::remove_dir_all(&backup);
        return Err(e);
    }
    Ok(backup)
}

/// Puts the previous bridge and `ui/` back after an update whose new bridge did not start. The launcher (this running
/// program on Windows and macOS) is left as it is: it is what is working. Returns false if there was nothing to restore.
pub fn restore_previous(install: &Path) -> bool {
    let backup = install.join(".update-previous");
    if !backup.is_dir() || !backup.join(bridge_name()).is_file() {
        return false;
    }
    for it in [Item { name: bridge_name(), dir: false }, Item { name: "ui", dir: true }] {
        let old = backup.join(it.name);
        if !old.exists() {
            continue;
        }
        let cur = install.join(it.name);
        remove_path(&cur, it.dir);
        let _ = std::fs::rename(&old, &cur);
    }
    true
}

/// Removes the previous copy (`<install>/.update-previous`) once the new bridge has started, and any temporary folder
/// left next to the install by an update that was killed. Harmless when there is nothing to remove.
pub fn cleanup_previous(install: &Path) {
    let _ = std::fs::remove_dir_all(install.join(".update-previous"));
    let (Some(parent), Some(name)) = (install.parent(), install.file_name()) else { return };
    let prefix = format!(".{}-update-", name.to_string_lossy());
    if let Ok(rd) = std::fs::read_dir(parent) {
        for entry in rd.flatten() {
            if entry.file_name().to_string_lossy().starts_with(&prefix) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
}

/// The note lives in the install folder, except inside a macOS app bundle, where macOS seals `Contents/Resources`:
/// the launcher sets MEGATRON_NOTE_DIR to a folder outside the bundle, so writing and later removing the note does
/// not invalidate the signature. Both the updater and the bridge read the same variable.
fn note_path(install: &Path) -> PathBuf {
    let dir = std::env::var_os("MEGATRON_NOTE_DIR").map(PathBuf::from).unwrap_or_else(|| install.to_path_buf());
    dir.join(".update-note.json")
}

/// Writes the note the page shows, replacing any earlier one. Atomic, so the bridge never reads a half-written file.
pub fn write_note(install: &Path, kind: &str, version: &str, reason: &str) -> Result<(), String> {
    let note = serde_json::json!({"kind": kind, "version": version, "reason": reason});
    let tmp = note_path(install).with_file_name(".update-note.json.tmp");
    std::fs::write(&tmp, note.to_string()).map_err(|e| format!("could not write the update note: {e}"))?;
    std::fs::rename(&tmp, note_path(install)).map_err(|e| format!("could not write the update note: {e}"))
}

pub fn clear_note(install: &Path) {
    let _ = std::fs::remove_file(note_path(install));
    let _ = std::fs::remove_file(note_path(install).with_file_name(".update-note.json.tmp"));
}

/// The note for the page, if there is one. Reading it removes it, so it is shown once (the launcher writes it again
/// on the next start if the situation is still true).
pub fn take_note(install: &Path) -> Option<Value> {
    let data = std::fs::read(note_path(install)).ok()?;
    let _ = std::fs::remove_file(note_path(install));
    let v: Value = serde_json::from_slice(&data).ok()?;
    v.is_object().then_some(v)
}

fn available(install: &Path, version: &str, reason: String) -> Outcome {
    let _ = write_note(install, "available", version, &reason);
    Outcome::Available { version: version.to_string(), reason }
}

/// The whole update: check, download, verify, unpack, swap. `current` and `public_key_b64` are what the build embedded.
/// Network failures before a newer version is known are returned as `Err`; everything after that (a missing build, a
/// bad signature, an unpack or rename failure) becomes `Available` with the reason, so the page can say why.
pub fn run(relay: &str, install: &Path, current: &str, public_key_b64: &str) -> Result<Outcome, String> {
    run_within(relay, install, current, public_key_b64, Some(Duration::from_secs(4)))
}

/// [`run`] for the page's Update button: the person is waiting for it, so a slow connection is allowed to finish.
pub fn run_now(relay: &str, install: &Path, current: &str, public_key_b64: &str) -> Result<Outcome, String> {
    run_within(relay, install, current, public_key_b64, None)
}

fn run_within(relay: &str, install: &Path, current: &str, public_key_b64: &str, limit: Option<Duration>) -> Result<Outcome, String> {
    let started = std::time::Instant::now();
    let late = || limit.is_some_and(|l| started.elapsed() > l);
    if !install.is_dir() {
        return Err(format!("{} is not an install folder", install.display()));
    }
    let Some(manifest) = crate::update::fetch_manifest(relay)? else {
        clear_note(install);
        return Ok(Outcome::None);
    };
    let rel = match release(&manifest, current) {
        Ok(Some(r)) => r,
        Ok(None) => {
            clear_note(install);
            return Ok(Outcome::None);
        }
        Err(reason) => {
            let v = manifest.get("version").and_then(Value::as_str).unwrap_or("").to_string();
            return Ok(available(install, &v, reason));
        }
    };
    // The launcher allows about five seconds. Stop before the download and before the swap rather than being killed
    // in the middle of the renames: a half-applied update is worse than a plain "try again next time".
    let too_long = || available(install, &rel.version, "the update took too long here; it will try again next time".into());
    if late() {
        return Ok(too_long());
    }
    let bytes = match download(&rel.url) {
        Ok(b) => b,
        Err(e) => return Ok(available(install, &rel.version, e)),
    };
    if let Err(e) = verify(&bytes, &rel.signature, public_key_b64, &rel.version) {
        return Ok(available(install, &rel.version, e));
    }
    let tmp = match temp_next_to(install) {
        Ok(t) => t,
        Err(e) => return Ok(available(install, &rel.version, e)),
    };
    let root = match unpack(&bytes, &tmp.0) {
        Ok(r) => r,
        Err(e) => return Ok(available(install, &rel.version, e)),
    };
    if late() {
        return Ok(too_long());
    }
    if let Err(e) = swap(install, &root) {
        return Ok(available(install, &rel.version, e));
    }
    // The old bridge process is still running from the file now in .update-previous; the launcher starts the new one
    // after this exits, and that new bridge removes .update-previous once it has bound its port.
    drop(tmp);
    let _ = write_note(install, "updated", &rel.version, "");
    Ok(Outcome::Updated { version: rel.version })
}

/// What the page's Update button shows: is something newer published for this system? Changes nothing.
pub fn check(relay: &str, current: &str) -> Result<Value, String> {
    let none = serde_json::json!({"current": current, "available": false});
    let Some(manifest) = crate::update::fetch_manifest(relay)? else { return Ok(none) };
    Ok(match release(&manifest, current) {
        Ok(Some(r)) => serde_json::json!({"current": current, "available": true, "version": r.version}),
        Ok(None) => none,
        Err(reason) => serde_json::json!({"current": current, "available": false, "reason": reason}),
    })
}

/// An update replaced files inside an ad-hoc signed macOS bundle, so its seal no longer matches. Re-signing keeps
/// Gatekeeper happy on the next launch; codesign is part of macOS itself. Failure is not fatal (the app still runs).
/// Only a bundle layout is re-signed: install is Megatron.app/Contents/Resources.
#[cfg(target_os = "macos")]
pub fn resign_bundle(install: &Path) {
    use std::process::{Command, Stdio};
    let Some(contents) = install.parent() else { return };
    let Some(app) = contents.parent() else { return };
    if contents.file_name().and_then(|n| n.to_str()) != Some("Contents")
        || install.file_name().and_then(|n| n.to_str()) != Some("Resources")
        || app.extension().and_then(|e| e.to_str()) != Some("app")
    {
        return;
    }
    let _ = Command::new("codesign")
        .args(["--force", "--deep", "--sign", "-"])
        .arg(app)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// A short line for the log / the launcher.
pub fn describe(outcome: &Outcome) -> String {
    match outcome {
        Outcome::None => "no update".into(),
        Outcome::Updated { version } => format!("updated to {version}"),
        Outcome::Available { version, reason } => format!("update available ({version}): {reason}"),
    }
}

/// Unix seconds, for the test signer's trusted comment.
pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::json;

    struct TestKey {
        id: [u8; 8],
        signing: SigningKey,
        /// base64 of the minisign `.pub` text, as it is embedded in the build.
        embedded: String,
    }

    impl TestKey {
        /// A throwaway key pair, made fresh in each test; the owner's real key is never involved.
        fn new(seed: u8) -> Self {
            let signing = SigningKey::from_bytes(&[seed; 32]);
            let id = [seed.wrapping_mul(7); 8];
            let mut bin = Vec::with_capacity(42);
            bin.extend_from_slice(&[0x45, 0x44]);
            bin.extend_from_slice(&id);
            bin.extend_from_slice(signing.verifying_key().as_bytes());
            use base64::Engine;
            let pub_text = format!(
                "untrusted comment: minisign public key: {}\n{}\n",
                bin[2..10].iter().map(|b| format!("{b:02X}")).collect::<String>(),
                base64::engine::general_purpose::STANDARD.encode(&bin)
            );
            Self { id, signing, embedded: base64::engine::general_purpose::STANDARD.encode(pub_text.as_bytes()) }
        }

        fn sign(&self, data: &[u8], version: &str) -> String {
            use base64::Engine;
            let e = base64::engine::general_purpose::STANDARD;
            let mut h = [0u8; 64];
            {
                use blake2::Digest;
                let mut hasher = blake2::Blake2b512::new();
                hasher.update(data);
                h.copy_from_slice(&hasher.finalize());
            }
            let sig: [u8; 64] = self.signing.sign(&h).to_bytes();
            let mut bin1 = Vec::with_capacity(74);
            bin1.extend_from_slice(&[0x45, 0x44]);
            bin1.extend_from_slice(&self.id);
            bin1.extend_from_slice(&sig);
            let trusted = format!("timestamp:{}\tfile:test\tprehashed\tversion:{version}", now_secs());
            let mut global = Vec::with_capacity(64 + trusted.len());
            global.extend_from_slice(&sig);
            global.extend_from_slice(trusted.as_bytes());
            let global = self.signing.sign(&global).to_bytes();
            let text = format!(
                "untrusted comment: signature from minisign secret key\n{}\ntrusted comment: {}\n{}\n",
                e.encode(&bin1),
                trusted,
                e.encode(global)
            );
            // The manifest holds base64 of the .sig text, exactly as the Tauri signer's file is passed to the apps.
            e.encode(text.as_bytes())
        }
    }

    fn manifest(version: &str, signature: &str) -> Value {
        json!({"version": version, "platforms": {PLATFORM: {"url": "update/Megatron-linux-browser.tar.gz", "signature": signature}}})
    }

    #[test]
    fn signature_accepts_only_a_signed_file_for_that_version() {
        let key = TestKey::new(1);
        let other = TestKey::new(2);
        let data = b"the update tarball";
        assert!(verify(data, &key.sign(data, "0.2.4"), &key.embedded, "0.2.4").is_ok());
        assert!(verify(b"tampered", &key.sign(data, "0.2.4"), &key.embedded, "0.2.4").is_err(), "a changed byte is refused");
        assert!(verify(data, &other.sign(data, "0.2.4"), &key.embedded, "0.2.4").is_err(), "another key is refused");
        assert!(verify(data, &key.sign(data, "0.2.3"), &key.embedded, "0.2.4").is_err(), "a signature for another version is refused");
        assert!(verify(data, "", &key.embedded, "0.2.4").is_err(), "an unsigned file is refused");
        assert!(verify(data, "not base64!", &key.embedded, "0.2.4").is_err());
        assert!(verify(data, &key.sign(data, "0.2.4"), "", "0.2.4").is_err(), "a build without a key refuses everything");
        assert!(verify(data, &key.sign(data, "0.2.4"), &other.embedded, "0.2.4").is_err());
    }

    #[test]
    fn only_a_newer_version_is_offered() {
        let key = TestKey::new(3);
        let sig = key.sign(b"x", "0.2.4");
        assert_eq!(release(&manifest("0.2.4", &sig), "0.2.3").unwrap().unwrap().version, "0.2.4");
        assert!(release(&manifest("0.2.3", &sig), "0.2.3").unwrap().is_none(), "the same version is not an update");
        assert!(release(&manifest("0.2.2", &sig), "0.2.3").unwrap().is_none(), "a downgrade is refused");
        assert!(release(&manifest("not-a-version", &sig), "0.2.3").unwrap().is_none());
        assert!(release(&json!({"version": "0.2.4"}), "0.2.3").unwrap_err().contains("no browser version"));
        assert!(release(&json!({"version": "0.2.4", "platforms": {PLATFORM: {"url": "update/x"}}}), "0.2.3").unwrap_err().contains("signature"));
        assert!(release(&json!({"version": "0.2.4", "platforms": {PLATFORM: {"signature": "s"}}}), "0.2.3").is_err());
        // A desktop platform alone must not be taken for the browser version.
        let m = json!({"version": "0.2.4", "platforms": {"linux-x86_64": {"url": "update/a", "signature": "s"}}});
        assert!(release(&m, "0.2.3").unwrap_err().contains("no browser version"));
    }

    /// Builds a tarball by writing the header names directly, so the tests can also forge the hostile paths the
    /// builder itself would refuse to write.
    fn tarball(files: &[(&str, &[u8], bool)]) -> Vec<u8> {
        tar_gz(files, &[])
    }

    fn tar_gz(files: &[(&str, &[u8], bool)], symlinks: &[(&str, &str)]) -> Vec<u8> {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
        {
            let mut b = tar::Builder::new(&mut gz);
            for (name, data, exec) in files {
                let mut h = tar::Header::new_gnu();
                let n = name.as_bytes();
                h.as_old_mut().name[..n.len()].copy_from_slice(n);
                h.as_old_mut().linkflag[0] = b'0';
                h.set_size(data.len() as u64);
                h.set_mode(if *exec { 0o755 } else { 0o644 });
                h.set_mtime(0);
                h.set_cksum();
                b.append(&h, &data[..]).unwrap();
            }
            for (name, target) in symlinks {
                let mut h = tar::Header::new_gnu();
                let n = name.as_bytes();
                h.as_old_mut().name[..n.len()].copy_from_slice(n);
                let t = target.as_bytes();
                h.as_old_mut().linkname[..t.len()].copy_from_slice(t);
                h.as_old_mut().linkflag[0] = b'2';
                h.set_size(0);
                h.set_mode(0o777);
                h.set_mtime(0);
                h.set_cksum();
                b.append(&h, &[][..]).unwrap();
            }
            b.finish().unwrap();
        }
        gz.finish().unwrap()
    }

    #[test]
    fn unpack_accepts_a_browser_tarball_and_refuses_unsafe_ones() {
        let dir = tempfile::tempdir().unwrap();
        let good = tarball(&[("megatron-browser/bridge", b"#!/bin/sh\n", true), ("megatron-browser/ui/index.html", b"<html>", false)]);
        let root = unpack(&good, dir.path()).unwrap();
        assert!(root.join("bridge").is_file());
        assert!(root.join("ui/index.html").is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(root.join("bridge")).unwrap().permissions().mode() & 0o777, 0o755);
            assert_eq!(std::fs::metadata(root.join("ui/index.html")).unwrap().permissions().mode() & 0o777, 0o644);
        }

        for bad in ["megatron-browser/../../evil", "/etc/passwd", "other/bridge"] {
            let d = tempfile::tempdir().unwrap();
            let bytes = tarball(&[(bad, b"x", false), ("megatron-browser/bridge", b"b", true), ("megatron-browser/ui/i", b"i", false)]);
            assert!(unpack(&bytes, d.path()).is_err(), "{bad} must be refused");
        }
        let d = tempfile::tempdir().unwrap();
        let links = tar_gz(
            &[("megatron-browser/bridge", b"b", true), ("megatron-browser/ui/i", b"i", false)],
            &[("megatron-browser/ui/link", "/etc/passwd")],
        );
        assert!(unpack(&links, d.path()).is_err(), "a symlink is refused");

        let d = tempfile::tempdir().unwrap();
        assert!(unpack(&tarball(&[("megatron-browser/ui/i", b"i", false)]), d.path()).is_err(), "a tarball without a bridge is refused");
    }

    fn install_with(dir: &Path, ui: &str, bridge: &str) {
        std::fs::create_dir_all(dir.join("ui")).unwrap();
        std::fs::write(dir.join("ui/index.html"), ui).unwrap();
        std::fs::write(dir.join("bridge"), bridge).unwrap();
    }

    fn new_with(dir: &Path, ui: &str, bridge: &str) {
        std::fs::create_dir_all(dir.join("ui")).unwrap();
        std::fs::write(dir.join("ui/index.html"), ui).unwrap();
        std::fs::write(dir.join("bridge"), bridge).unwrap();
    }

    #[test]
    fn swap_puts_the_new_files_in_place_and_keeps_the_old_until_cleanup() {
        let d = tempfile::tempdir().unwrap();
        let inst = d.path().join("install");
        let new = d.path().join("new");
        install_with(&inst, "old-ui", "old-bridge");
        new_with(&new, "new-ui", "new-bridge");
        let backup = swap(&inst, &new).unwrap();
        assert_eq!(std::fs::read_to_string(inst.join("ui/index.html")).unwrap(), "new-ui");
        assert_eq!(std::fs::read_to_string(inst.join("bridge")).unwrap(), "new-bridge");
        assert_eq!(std::fs::read_to_string(backup.join("ui/index.html")).unwrap(), "old-ui");
        assert_eq!(std::fs::read_to_string(backup.join("bridge")).unwrap(), "old-bridge");
        cleanup_previous(&inst);
        assert!(!inst.join(".update-previous").exists(), "the old copy is removed once the new bridge starts");
    }

    #[test]
    fn each_system_reads_only_its_own_browser_key() {
        let key = TestKey::new(4);
        let sig = key.sign(b"x", "0.2.4");
        let manifest = json!({"version": "0.2.4", "platforms": {
            "linux-browser-x86_64": {"url": "update/linux.tar.gz", "signature": sig},
            "windows-browser-x86_64": {"url": "update/windows.tar.gz", "signature": sig},
            "darwin-browser-aarch64": {"url": "update/mac.tar.gz", "signature": sig},
            "windows-x86_64": {"url": "update/desktop.exe", "signature": "desktop"},
        }});
        let pick = |p: &str| release_for(&manifest, "0.2.3", p).unwrap().unwrap().url;
        assert_eq!(pick("linux-browser-x86_64"), "update/linux.tar.gz");
        assert_eq!(pick("windows-browser-x86_64"), "update/windows.tar.gz");
        assert_eq!(pick("darwin-browser-aarch64"), "update/mac.tar.gz");
        // A manifest with only desktop targets has no browser version for any of the three keys.
        let desktops = json!({"version": "0.2.4", "platforms": {
            "linux-x86_64": {"url": "update/linux.AppImage", "signature": "s"},
            "windows-x86_64": {"url": "update/windows.exe", "signature": "s"},
            "darwin-aarch64": {"url": "update/mac.app.tar.gz", "signature": "s"},
        }});
        for p in ["linux-browser-x86_64", "windows-browser-x86_64", "darwin-browser-aarch64"] {
            assert!(release_for(&desktops, "0.2.3", p).is_err(), "{p} must not take a desktop entry");
        }
    }

    #[test]
    fn an_update_that_carries_a_launcher_replaces_it_in_the_same_transaction() {
        let d = tempfile::tempdir().unwrap();
        let inst = d.path().join("install");
        let new = d.path().join("new");
        for (root, ui, bridge, launcher) in [(&inst, "old-ui", "old-bridge", "old-launcher"), (&new, "new-ui", "new-bridge", "new-launcher")] {
            install_with(root, ui, bridge);
            std::fs::write(root.join(launcher_name()), launcher).unwrap();
        }
        let backup = swap(&inst, &new).unwrap();
        assert_eq!(std::fs::read_to_string(inst.join("ui/index.html")).unwrap(), "new-ui");
        assert_eq!(std::fs::read_to_string(inst.join(bridge_name())).unwrap(), "new-bridge");
        assert_eq!(std::fs::read_to_string(inst.join(launcher_name())).unwrap(), "new-launcher");
        assert_eq!(std::fs::read_to_string(backup.join(launcher_name())).unwrap(), "old-launcher");
        cleanup_previous(&inst);
        assert!(!inst.join(".update-previous").exists());
    }

    #[test]
    fn a_failed_launcher_swap_rolls_back_everything() {
        for fail_after in [0usize, 1, 2, 3, 4, 5] {
            let d = tempfile::tempdir().unwrap();
            let inst = d.path().join("install");
            let new = d.path().join("new");
            for (root, ui, bridge, launcher) in [(&inst, "old-ui", "old-bridge", "old-launcher"), (&new, "new-ui", "new-bridge", "new-launcher")] {
                install_with(root, ui, bridge);
                std::fs::write(root.join(launcher_name()), launcher).unwrap();
            }
            assert!(swap_inner(&inst, &new, Some(fail_after)).is_err(), "fail_after={fail_after}");
            assert_eq!(std::fs::read_to_string(inst.join("ui/index.html")).unwrap(), "old-ui", "fail_after={fail_after}");
            assert_eq!(std::fs::read_to_string(inst.join(bridge_name())).unwrap(), "old-bridge", "fail_after={fail_after}");
            assert_eq!(std::fs::read_to_string(inst.join(launcher_name())).unwrap(), "old-launcher", "fail_after={fail_after}");
            assert!(!inst.join(".update-previous").exists(), "fail_after={fail_after}");
        }
    }

    #[test]
    fn restore_previous_puts_back_the_bridge_and_ui_but_leaves_the_running_launcher() {
        let d = tempfile::tempdir().unwrap();
        let inst = d.path().join("install");
        let new = d.path().join("new");
        for (root, ui, bridge, launcher) in [(&inst, "old-ui", "old-bridge", "old-launcher"), (&new, "new-ui", "new-bridge", "new-launcher")] {
            install_with(root, ui, bridge);
            std::fs::write(root.join(launcher_name()), launcher).unwrap();
        }
        swap(&inst, &new).unwrap();
        assert!(restore_previous(&inst));
        assert_eq!(std::fs::read_to_string(inst.join("ui/index.html")).unwrap(), "old-ui");
        assert_eq!(std::fs::read_to_string(inst.join(bridge_name())).unwrap(), "old-bridge");
        assert_eq!(std::fs::read_to_string(inst.join(launcher_name())).unwrap(), "new-launcher", "the working launcher stays");
        cleanup_previous(&inst);
        assert!(!inst.join(".update-previous").exists());
    }

    #[test]
    fn a_failed_swap_rolls_back_completely() {
        for fail_after in [0usize, 1, 2, 3] {
            let d = tempfile::tempdir().unwrap();
            let inst = d.path().join("install");
            let new = d.path().join("new");
            install_with(&inst, "old-ui", "old-bridge");
            new_with(&new, "new-ui", "new-bridge");
            let e = swap_inner(&inst, &new, Some(fail_after));
            assert!(e.is_err(), "fail_after={fail_after}");
            assert_eq!(std::fs::read_to_string(inst.join("ui/index.html")).unwrap(), "old-ui", "fail_after={fail_after}");
            assert_eq!(std::fs::read_to_string(inst.join("bridge")).unwrap(), "old-bridge", "fail_after={fail_after}");
            assert!(!inst.join(".update-previous").exists(), "fail_after={fail_after}");
        }
    }

    #[test]
    fn the_note_is_shown_once_and_cleared_by_a_check_with_nothing_newer() {
        let d = tempfile::tempdir().unwrap();
        write_note(d.path(), "updated", "0.2.4", "").unwrap();
        let n = take_note(d.path()).unwrap();
        assert_eq!(n["kind"], "updated");
        assert_eq!(n["version"], "0.2.4");
        assert!(take_note(d.path()).is_none(), "the page sees it once");
        write_note(d.path(), "available", "0.2.5", "signature bad").unwrap();
        clear_note(d.path());
        assert!(take_note(d.path()).is_none());
    }

    #[test]
    fn run_clears_a_stale_note_when_the_server_publishes_nothing_newer() {
        // A loopback server that says the version is already the one running: no change, and the old note goes.
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for c in listener.incoming().take(2) {
                let Ok(mut c) = c else { continue };
                let mut buf = [0u8; 4096]; let _ = c.read(&mut buf);
                let body = r#"{"version":"0.2.3","platforms":{"linux-browser-x86_64":{"url":"update/a","signature":"s"}}}"#;
                let _ = write!(c, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}", body.len(), body);
            }
        });
        let d = tempfile::tempdir().unwrap();
        write_note(d.path(), "updated", "0.2.3", "").unwrap();
        let out = run(&format!("http://{addr}"), d.path(), "0.2.3", "").unwrap();
        assert_eq!(out, Outcome::None);
        assert!(take_note(d.path()).is_none());
    }
}
