//! What the app checks about an update before the Tauri updater plugin downloads anything. The plugin itself checks the
//! signature of the file against the public key built into the app, and that the signature was made for the version the
//! manifest announces; this module makes sure the manifest and the file come from the relay this account uses, and
//! from nowhere else. See docs/UPDATES.md.
//!
//! The manifest lives at `<relay>/update/latest.json`. Its file addresses are written relative to the relay
//! (`update/Commons_0.2.1_amd64.AppImage`), so one manifest serves whichever address friends reach the relay at. An
//! absolute address is accepted only if it points into the same relay; anything else is refused before a byte of it is
//! fetched. Redirects are not followed.
use serde_json::Value;
use std::io::Read;
use std::time::Duration;

/// A manifest is a few hundred bytes; anything this big is not one.
pub const MAX_MANIFEST_BYTES: u64 = 256 * 1024;

/// The relay address an update may come from: https, or plain http only on this computer (local tests and
/// `scripts/run-local.sh`). `relay` is the account's relay as stored, `http(s)://host[:port][/path]`.
pub fn relay_base(relay: &str) -> Result<String, String> {
    let r = crate::api::clean_relay_url(relay)?;
    if let Some(rest) = r.strip_prefix("http://") {
        let host = rest.split('/').next().unwrap_or("");
        let name = host.rsplit_once(':').filter(|(_, p)| p.parse::<u16>().is_ok()).map(|(h, _)| h).unwrap_or(host);
        if !matches!(name, "127.0.0.1" | "localhost") { return Err("this relay does not use https, so updates are not taken from it".into()); }
    }
    Ok(r)
}

/// Where the manifest is, for this relay.
pub fn manifest_url(relay: &str) -> Result<String, String> { Ok(format!("{}/update/latest.json", relay_base(relay)?)) }

/// A path inside the relay: plain names and folders, nothing that could climb out or carry a query.
fn plain_path(p: &str) -> bool {
    !p.is_empty() && p.len() <= 512 && p.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_' | b'/' | b'+'))
        && p.split('/').all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

/// One file address from the manifest, made absolute against the relay, or refused if it points anywhere else.
pub fn resolve(url: &str, relay: &str) -> Result<String, String> {
    let base = relay_base(relay)?;
    let rel = if url.contains("://") {
        url.strip_prefix(&format!("{base}/")).ok_or_else(|| format!("the update manifest points at another server ({url}); refused"))?
    } else { url };
    if !plain_path(rel) { return Err(format!("the update manifest has an unusable file address ({url}); refused")); }
    Ok(format!("{base}/{rel}"))
}

/// Rewrites every file address in a manifest (`url`, and `platforms.*.url`) to its absolute form on this relay.
/// Fails if any of them would lead somewhere else, so a manifest that mentions another server is refused whole.
pub fn localize(manifest: &mut Value, relay: &str) -> Result<(), String> {
    if !manifest.is_object() { return Err("the update manifest is not a JSON object".into()); }
    if let Some(u) = manifest.get("url").cloned() {
        manifest["url"] = Value::String(resolve(u.as_str().ok_or("bad url in the update manifest")?, relay)?);
    }
    if let Some(ps) = manifest.get_mut("platforms") {
        let ps = ps.as_object_mut().ok_or("bad platforms in the update manifest")?;
        for (_, p) in ps.iter_mut() {
            let u = p.get("url").and_then(|u| u.as_str()).ok_or("a platform in the update manifest has no url")?.to_string();
            p["url"] = Value::String(resolve(&u, relay)?);
        }
    }
    Ok(())
}

/// `major.minor.patch[-pre]`, numbers compared as numbers; a pre-release sorts before its release.
pub fn parse_version(v: &str) -> Option<(u64, u64, u64, Option<String>)> {
    let v = v.trim().trim_start_matches('v');
    let (core, pre) = match v.split_once('-') { Some((c, p)) => (c, Some(p.to_string())), None => (v.split('+').next().unwrap_or(v), None) };
    let mut it = core.split('.').map(|x| x.parse::<u64>().ok());
    let (a, b, c) = (it.next()??, it.next()??, it.next()??);
    if it.next().is_some() { return None; }
    Some((a, b, c, pre))
}
pub fn version_lt(a: &str, b: &str) -> Option<bool> {
    let (x, y) = (parse_version(a)?, parse_version(b)?);
    Some(match (x.0, x.1, x.2).cmp(&(y.0, y.1, y.2)) {
        std::cmp::Ordering::Less => true,
        std::cmp::Ordering::Greater => false,
        std::cmp::Ordering::Equal => match (&x.3, &y.3) { (Some(p), Some(q)) => p < q, (Some(_), None) => true, _ => false },
    })
}

/// The oldest version the relay still wants to see (`minimum_version` in the manifest, which the relay's operator sets:
/// `commons-relay --min-app-version`, or `scripts/publish-update.sh --min-version`). Nothing is refused because of it;
/// an older app is told, every time it starts, that it has to update.
pub fn minimum(manifest: &Value) -> Option<String> {
    manifest.get("minimum_version").and_then(|v| v.as_str()).filter(|v| parse_version(v).is_some()).map(String::from)
}
pub fn below_minimum(current: &str, manifest: &Value) -> bool {
    minimum(manifest).and_then(|m| version_lt(current, &m)).unwrap_or(false)
}

/// Fetches and checks the manifest. `Ok(None)`: this relay publishes no updates (an older relay, or nothing published).
pub fn fetch_manifest(relay: &str) -> Result<Option<Value>, String> {
    let url = manifest_url(relay)?;
    let agent = ureq::AgentBuilder::new().redirects(0).timeout_connect(Duration::from_secs(8)).timeout(Duration::from_secs(20)).build();
    let resp = match agent.get(&url).set("accept", "application/json").call() {
        Ok(r) => match r.status() {
            200 => r,
            300..=399 => return Err("the relay sent the update check somewhere else; refused".into()),
            _ => return Ok(None),
        },
        Err(ureq::Error::Status(400..=499, _)) => return Ok(None),   // an older relay has no such route
        Err(e) => return Err(format!("could not check for updates: {e}")),
    };
    let mut body = Vec::new();
    resp.into_reader().take(MAX_MANIFEST_BYTES + 1).read_to_end(&mut body).map_err(|e| format!("could not read the update manifest: {e}"))?;
    if body.len() as u64 > MAX_MANIFEST_BYTES { return Err("the update manifest is too large; refused".into()); }
    let mut v: Value = serde_json::from_slice(&body).map_err(|e| format!("the update manifest is not valid JSON: {e}"))?;
    localize(&mut v, relay)?;
    Ok(Some(v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    const R: &str = "https://chat.example.org:10000";

    #[test]
    fn only_https_or_this_computer() {
        assert_eq!(manifest_url(R).unwrap(), "https://chat.example.org:10000/update/latest.json");
        assert_eq!(manifest_url("https://chat.example.org/commons/").unwrap(), "https://chat.example.org/commons/update/latest.json");
        assert!(manifest_url("http://127.0.0.1:18787").is_ok());
        assert!(manifest_url("http://localhost:18787").is_ok());
        assert!(manifest_url("http://chat.example.org").is_err(), "plain http over a network is refused");
        assert!(manifest_url("http://127.0.0.1.evil.org").is_err());
        assert!(manifest_url("ftp://x").is_err());
    }

    #[test]
    fn file_addresses_stay_on_the_relay() {
        assert_eq!(resolve("update/Commons_0.2.1_amd64.AppImage", R).unwrap(), format!("{R}/update/Commons_0.2.1_amd64.AppImage"));
        assert_eq!(resolve(&format!("{R}/download/Commons-windows-x64-setup.exe"), R).unwrap(), format!("{R}/download/Commons-windows-x64-setup.exe"));
        assert_eq!(resolve("update/a.tar.gz", "https://h.org/commons").unwrap(), "https://h.org/commons/update/a.tar.gz");
        for bad in ["https://evil.example.org/update/x.AppImage", "https://chat.example.org:10001/update/x", "https://chat.example.org:100000/x",
                    "https://chat.example.org:10000@evil.org/x", "http://chat.example.org:10000/update/x", "//evil.org/x", "/update/x", "../x",
                    "update/../../x", "update/x?y=1", "update/x#y", "update//x", "update/x y", "update\\x", "", "file:///etc/passwd"] {
            assert!(resolve(bad, R).is_err(), "{bad} must be refused");
        }
        assert!(resolve("https://h.org/other/update/x", "https://h.org/commons").is_err(), "another path on the same host is not this relay");
    }

    #[test]
    fn a_manifest_mentioning_another_server_is_refused_whole() {
        let mut m = json!({"version": "0.2.1", "platforms": {"linux-x86_64": {"url": "update/a.AppImage", "signature": "s"}, "windows-x86_64": {"url": "https://evil.org/b.exe", "signature": "s"}}});
        assert!(localize(&mut m, R).unwrap_err().contains("another server"));
        let mut m = json!({"version": "0.2.1", "platforms": {"linux-x86_64": {"url": "update/a.AppImage", "signature": "s"}}});
        localize(&mut m, R).unwrap();
        assert_eq!(m["platforms"]["linux-x86_64"]["url"], format!("{R}/update/a.AppImage"));
        let mut m = json!({"version": "0.2.1", "url": "https://evil.org/x", "signature": "s"});
        assert!(localize(&mut m, R).is_err());
        assert!(localize(&mut json!([1]), R).is_err());
        assert!(localize(&mut json!({"platforms": {"x": {"signature": "s"}}}), R).is_err());
    }

    /// A one-shot HTTP server on loopback that answers every request with `reply`.
    fn serve(reply: &'static str) -> String {
        use std::io::Write;
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || for c in l.incoming().take(4) {
            let Ok(mut c) = c else { continue };
            let mut buf = [0u8; 4096]; let _ = c.read(&mut buf);
            let _ = c.write_all(reply.as_bytes());
        });
        format!("http://{addr}")
    }

    #[test]
    fn the_manifest_is_fetched_from_the_relay_and_nowhere_else() {
        let ok = serve("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 84\r\nconnection: close\r\n\r\n{\"version\":\"9.9.9\",\"platforms\":{\"linux-x86_64\":{\"url\":\"update/a\",\"signature\":\"c2\"}}}");
        let m = fetch_manifest(&ok).unwrap().unwrap();
        assert_eq!(m["platforms"]["linux-x86_64"]["url"], format!("{ok}/update/a"));
        let moved = serve("HTTP/1.1 302 Found\r\nlocation: https://evil.example.org/latest.json\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
        assert!(fetch_manifest(&moved).unwrap_err().contains("somewhere else"), "a redirect is not followed");
        let none = serve("HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
        assert_eq!(fetch_manifest(&none).unwrap(), None, "an older relay without updates is not an error");
        let elsewhere = serve("HTTP/1.1 200 OK\r\ncontent-length: 94\r\nconnection: close\r\n\r\n{\"version\":\"9.9.9\",\"platforms\":{\"linux-x86_64\":{\"url\":\"https://evil.org/a\",\"signature\":\"c2\"}}}");
        assert!(fetch_manifest(&elsewhere).unwrap_err().contains("another server"));
    }

    #[test]
    fn versions_and_the_minimum() {
        assert_eq!(version_lt("0.1.0", "0.2.0"), Some(true));
        assert_eq!(version_lt("0.10.0", "0.9.9"), Some(false));
        assert_eq!(version_lt("0.2.0-rc1", "0.2.0"), Some(true));
        assert_eq!(version_lt("0.2.0", "0.2.0"), Some(false));
        assert_eq!(version_lt("v1.0.0", "1.0.1"), Some(true));
        assert_eq!(version_lt("x", "1.0.0"), None);
        let m = json!({"version": "0.3.0", "minimum_version": "0.2.5"});
        assert!(below_minimum("0.2.0", &m));
        assert!(!below_minimum("0.2.5", &m));
        assert!(!below_minimum("0.2.0", &json!({"version": "0.3.0"})));
        assert!(!below_minimum("0.2.0", &json!({"minimum_version": "soon"})), "a minimum that is not a version is ignored");
    }
}
