//! Who may talk to the browser version's local bridge (`dev_bridge`, shipped as `bridge`).
//!
//! The bridge listens on a fixed loopback port, and loopback is shared by every user and program on the computer. Two layers:
//!
//! * **Secret mode.** The launcher gives the bridge a random per-start secret (`MEGATRON_SECRET`), keeps a copy in a file only
//!   its user can read, and opens the window at `http://127.0.0.1:<port>/#s=<secret>`. A URL fragment is never sent to a
//!   server, so it is in no request line or log. The page sends it back in the `X-Megatron-Secret` header on every `/rpc` and
//!   `/update-*` call; the bridge compares it in constant time. `GET /hello?c=<nonce>` lets the launcher check that the
//!   program on the port really knows the secret (it answers with a hash of secret and nonce), so a squatter is refused.
//! * **Open (legacy) mode.** No secret was given: an older launcher, the user's own script or service, a test rig. Nothing
//!   to present, so the old rules apply (Host and Origin checks), plus on Linux the connecting socket must belong to the same
//!   user (read from /proc/net/tcp). The page is told, and says so in plain words.
use sha2::{Digest, Sha256};
use std::path::Path;

pub const HEADER: &str = "x-megatron-secret";

/// The shortest secret the bridge accepts from its environment.
pub const MIN_SECRET: usize = 32;

#[derive(Clone)]
pub struct Auth {
    secret: Option<String>,
}

pub fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && !a.is_empty() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// A fresh 256-bit secret as hex.
pub fn new_secret() -> String {
    use rand::RngCore;
    let mut b = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut b);
    hex::encode(b)
}

/// What a bridge that knows `secret` answers to `GET /hello?c=<nonce>`. The launcher computes the same with sha256sum.
pub fn proof(secret: &str, nonce: &str) -> String {
    hex::encode(Sha256::digest(format!("megatron-bridge-v1|{secret}|{nonce}").as_bytes()))
}

fn valid_secret(s: &str) -> bool {
    s.len() >= MIN_SECRET && s.len() <= 256 && s.bytes().all(|c| c.is_ascii_alphanumeric())
}

impl Auth {
    /// Secret mode when `MEGATRON_SECRET` holds a usable secret, open mode otherwise.
    pub fn from_env() -> Auth {
        Auth { secret: std::env::var("MEGATRON_SECRET").ok().filter(|s| valid_secret(s)) }
    }
    pub fn with_secret(secret: &str) -> Auth { Auth { secret: valid_secret(secret).then(|| secret.to_string()) } }
    pub fn open() -> Auth { Auth { secret: None } }
    pub fn is_open(&self) -> bool { self.secret.is_none() }
    pub fn secret(&self) -> Option<&str> { self.secret.as_deref() }
    /// May this request use a protected route? Open mode lets everything through (the other checks still apply).
    pub fn allows(&self, presented: Option<&str>) -> bool {
        match &self.secret {
            None => true,
            Some(s) => presented.is_some_and(|p| ct_eq(p.trim(), s)),
        }
    }
    /// The body of `GET /hello?c=<nonce>`.
    pub fn hello(&self, nonce: &str) -> serde_json::Value {
        let nonce: String = nonce.chars().filter(|c| c.is_ascii_alphanumeric()).take(64).collect();
        match &self.secret {
            Some(s) => serde_json::json!({"megatron": 1, "mode": "secret", "proof": proof(s, &nonce), "pid": std::process::id()}),
            None => serde_json::json!({"megatron": 1, "mode": "open", "pid": std::process::id()}),
        }
    }
}

/// Writes a secret file only its owner can read (mode 600; on Windows the user's own profile folder already is private).
pub fn write_secret_file(path: &Path, secret: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    { use std::os::unix::fs::OpenOptionsExt; o.mode(0o600); }
    let mut f = o.open(&tmp)?;
    f.write_all(secret.as_bytes())?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}

pub fn read_secret_file(path: &Path) -> Option<String> {
    let s = std::fs::read_to_string(path).ok()?;
    let s = s.trim().to_string();
    valid_secret(&s).then_some(s)
}

/// On Linux, the user that owns the other end of a loopback connection, from /proc/net/tcp. `None` when it cannot be told
/// (other systems, no procfs, entry gone), which callers treat as allowed: this is a second layer, not the only one.
#[cfg(target_os = "linux")]
pub fn peer_is_same_user(peer: std::net::SocketAddr, local: std::net::SocketAddr) -> bool {
    use std::os::unix::fs::MetadataExt;
    let (std::net::SocketAddr::V4(p), std::net::SocketAddr::V4(l)) = (peer, local) else { return true };
    let Ok(me) = std::fs::metadata("/proc/self").map(|m| m.uid()) else { return true };
    let Ok(text) = std::fs::read_to_string("/proc/net/tcp") else { return true };
    let key = |a: &std::net::SocketAddrV4| format!("{:08X}:{:04X}", u32::from_le_bytes(a.ip().octets()), a.port());
    let (pk, lk) = (key(&p), key(&l));
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() > 7 && f[1] == pk && f[2] == lk {
            return f[7].parse::<u32>().map(|u| u == me).unwrap_or(true);
        }
    }
    true
}

#[cfg(not(target_os = "linux"))]
pub fn peer_is_same_user(_peer: std::net::SocketAddr, _local: std::net::SocketAddr) -> bool { true }

/// The file a request path names inside the `ui` folder, or None for anything that is not plain names below it. A path with a drive
/// letter, a backslash, a colon (also an NTFS stream), `..` or an absolute start must never reach `Path::join`: on Windows
/// `ui.join("C:/x")` replaces the base entirely.
pub fn ui_file(ui: &Path, url_path: &str) -> Option<std::path::PathBuf> {
    let p = url_path.split(['?', '#']).next().unwrap_or("");
    let p = p.strip_prefix('/').unwrap_or(p);
    let p = if p.is_empty() { "index.html" } else { p };
    let mut out = ui.to_path_buf();
    for seg in p.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." || seg.contains(['\\', ':', '\0']) || seg.ends_with(' ') || seg.ends_with('.') { return None; }
        out.push(seg);
    }
    // Symbolic links inside ui/ must not lead out of it either.
    if let (Ok(real), Ok(base)) = (out.canonicalize(), ui.canonicalize()) {
        if !real.starts_with(&base) { return None; }
    }
    Some(out)
}

/// The text of the Linux launcher this build ships, so a new bridge can replace an older launcher script in its own install.
pub const LINUX_LAUNCHER: &str = include_str!("../../../packaging/linux-browser/megatron");
/// A launcher that contains this line speaks the secret protocol; an older one does not.
pub const LAUNCHER_MARK: &str = "# megatron-launcher-protocol: 2";

/// Replaces `<install>/megatron` with the launcher this build ships when the one there is an older copy of ours
/// (looks like our script, lacks the protocol mark). The old file is kept as `megatron.old`. Returns true when replaced.
#[cfg(unix)]
pub fn refresh_linux_launcher(install: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let path = install.join("megatron");
    let Ok(meta) = std::fs::symlink_metadata(&path) else { return false };
    if !meta.file_type().is_file() { return false; }
    let Ok(cur) = std::fs::read_to_string(&path) else { return false };
    if cur.contains(LAUNCHER_MARK) || !cur.starts_with("#!") || !cur.contains("--app=") || !cur.contains("/bridge") { return false; }
    let tmp = install.join(".megatron.new");
    if std::fs::write(&tmp, LINUX_LAUNCHER).is_err() { return false; }
    let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
    let _ = std::fs::copy(&path, install.join("megatron.old"));
    std::fs::rename(&tmp, &path).is_ok()
}
#[cfg(not(unix))]
pub fn refresh_linux_launcher(_install: &Path) -> bool { false }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secret_mode_needs_the_header_and_open_mode_does_not() {
        let s = new_secret();
        let a = Auth::with_secret(&s);
        assert!(!a.allows(None) && !a.allows(Some("")) && !a.allows(Some("x")));
        assert!(!a.allows(Some(&s[..s.len() - 1])));
        assert!(a.allows(Some(&s)) && a.allows(Some(&format!(" {s} "))));
        assert!(Auth::open().allows(None));
        assert!(Auth::with_secret("short").is_open(), "a weak secret is not accepted");
    }
    #[test]
    fn the_hello_proof_matches_what_a_shell_computes() {
        // printf 'megatron-bridge-v1|%s|%s' abc 123 | sha256sum
        assert_eq!(proof("abc", "123"), hex::encode(Sha256::digest(b"megatron-bridge-v1|abc|123")));
        let s = new_secret();
        let h = Auth::with_secret(&s).hello("n0nce");
        assert_eq!(h["proof"], proof(&s, "n0nce"));
        assert!(Auth::open().hello("n0nce").get("proof").is_none());
    }
    #[test]
    fn the_file_server_only_names_files_below_ui() {
        let d = tempfile::tempdir().unwrap();
        let ui = d.path().join("ui");
        std::fs::create_dir_all(ui.join("vendor")).unwrap();
        std::fs::write(ui.join("index.html"), "x").unwrap();
        std::fs::write(ui.join("vendor/a.js"), "x").unwrap();
        std::fs::write(d.path().join("secret.txt"), "s").unwrap();
        assert_eq!(ui_file(&ui, "/").unwrap(), ui.join("index.html"));
        assert_eq!(ui_file(&ui, "/vendor/a.js?v=2").unwrap(), ui.join("vendor/a.js"));
        for bad in ["/../secret.txt", "/vendor/../../secret.txt", "//etc/passwd", "/C:/Users/x/file", "/c:\\windows\\win.ini", "/..\\secret.txt", "/vendor//a.js", "/a.js:stream", "/index.html.", "/x\0y", "/./index.html"] {
            assert!(ui_file(&ui, bad).is_none(), "{bad}");
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(d.path().join("secret.txt"), ui.join("link.txt")).unwrap();
            assert!(ui_file(&ui, "/link.txt").is_none(), "a link out of ui/ is refused");
        }
    }
    #[test]
    fn the_shipped_launcher_carries_the_protocol_mark_and_is_valid_shell() {
        assert!(LINUX_LAUNCHER.contains(LAUNCHER_MARK));
        assert!(LINUX_LAUNCHER.starts_with("#!/usr/bin/env bash\n"));
    }
    #[cfg(unix)]
    #[test]
    fn only_an_older_copy_of_our_own_launcher_is_replaced() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("megatron");
        // nothing there: nothing to do
        assert!(!refresh_linux_launcher(d.path()));
        // a launcher of ours without the mark is replaced, and kept as megatron.old
        let old = "#!/usr/bin/env bash\nexec brave --app=http://127.0.0.1:7420/ # $D/bridge\n";
        std::fs::write(&p, old).unwrap();
        assert!(refresh_linux_launcher(d.path()));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), LINUX_LAUNCHER);
        assert_eq!(std::fs::read_to_string(d.path().join("megatron.old")).unwrap(), old);
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o755);
        // the current one, the second time round, is left alone
        assert!(!refresh_linux_launcher(d.path()));
        // somebody's own script is never touched
        let mine = "#!/bin/sh\necho my own thing\n";
        std::fs::write(&p, mine).unwrap();
        assert!(!refresh_linux_launcher(d.path()));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), mine);
        // nor a symlink
        std::fs::remove_file(&p).unwrap();
        std::os::unix::fs::symlink("/bin/sh", &p).unwrap();
        assert!(!refresh_linux_launcher(d.path()));
    }
    #[cfg(unix)]
    #[test]
    fn the_secret_file_is_private_and_round_trips() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub").join(".bridge-secret-1");
        let s = new_secret();
        write_secret_file(&p, &s).unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(read_secret_file(&p).as_deref(), Some(s.as_str()));
        std::fs::write(&p, "short").unwrap();
        assert!(read_secret_file(&p).is_none());
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn a_loopback_peer_of_this_user_is_recognised() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let c = std::net::TcpStream::connect(l.local_addr().unwrap()).unwrap();
        let (s, peer) = l.accept().unwrap();
        assert!(peer_is_same_user(peer, s.local_addr().unwrap()));
        drop(c);
    }
}
