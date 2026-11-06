//! The only network surface of the client. A trait so tests can inject faults (duplicates, reordering, drops).
use anyhow::{anyhow, Result};
use commons_proto::*;
use ed25519_dalek::SigningKey;
use std::time::Duration;

pub struct HttpResp { pub status: u16, pub body: Vec<u8> }

pub trait Transport: Send + Sync {
    /// `key` signs the request when Some.
    fn request(&self, key: Option<&SigningKey>, method: &str, path: &str, body: &[u8], timeout: Duration) -> Result<HttpResp>;
}

pub struct HttpTransport { base: String, agent: ureq::Agent }
impl HttpTransport {
    pub fn new(base: &str) -> Self {
        HttpTransport { base: base.trim_end_matches('/').to_string(), agent: ureq::AgentBuilder::new().build() }
    }
}
impl HttpTransport { pub fn into_arc(self) -> std::sync::Arc<dyn Transport> { std::sync::Arc::new(self) } }
impl Transport for HttpTransport {
    fn request(&self, key: Option<&SigningKey>, method: &str, path: &str, body: &[u8], timeout: Duration) -> Result<HttpResp> {
        let url = format!("{}{}", self.base, path);
        let mut r = self.agent.request(method, &url).timeout(timeout);
        if let Some(k) = key { r = r.set(AUTH_HEADER, &sign_request(k, method, path, body)); }
        let res = if body.is_empty() && method == "GET" { r.call() } else { r.set("content-type", "application/octet-stream").send_bytes(body) };
        match res {
            Ok(resp) => {
                let status = resp.status();
                let mut b = Vec::new();
                std::io::Read::read_to_end(&mut resp.into_reader().take(MAX_BLOB_BYTES as u64 + 4096), &mut b)?;
                Ok(HttpResp { status, body: b })
            }
            Err(ureq::Error::Status(code, resp)) => {
                let mut b = Vec::new();
                let _ = std::io::Read::read_to_end(&mut resp.into_reader().take(4096), &mut b);
                Ok(HttpResp { status: code, body: b })
            }
            Err(e) => Err(anyhow!("network: {e}")),
        }
    }
}
use std::io::Read as _;

/// Typed relay calls on top of a Transport.
pub struct Api { pub t: std::sync::Arc<dyn Transport> }
const T: Duration = Duration::from_secs(20);
fn ok(r: HttpResp, what: &str) -> Result<HttpResp> {
    if r.status / 100 == 2 { Ok(r) } else { Err(anyhow!("{what}: relay said {} {}", r.status, String::from_utf8_lossy(&r.body))) }
}
impl Api {
    fn post<B: serde::Serialize>(&self, k: &SigningKey, path: &str, b: &B) -> Result<HttpResp> {
        let body = serde_json::to_vec(b)?;
        ok(self.t.request(Some(k), "POST", path, &body, T)?, path)
    }
    pub fn register(&self, k: &SigningKey, cert: &DeviceCert) -> Result<()> { self.post(k, "/v1/devices", &RegisterReq { cert: cert.clone() }).map(|_| ()) }
    pub fn revoke(&self, account: &str, device: &str, sig: &str) -> Result<()> {
        let body = serde_json::to_vec(&RevokeReq { account: account.into(), device: device.into(), sig: sig.into() })?;
        ok(self.t.request(None, "POST", "/v1/devices/revoke", &body, T)?, "revoke").map(|_| ())
    }
    pub fn devices(&self, k: &SigningKey, account: &str) -> Result<Vec<DeviceInfo>> {
        let r = ok(self.t.request(Some(k), "GET", &format!("/v1/accounts/{account}/devices"), &[], T)?, "devices")?;
        Ok(serde_json::from_slice::<DevicesResp>(&r.body)?.devices)
    }
    pub fn publish_kps(&self, k: &SigningKey, packages: Vec<Vec<u8>>, last_resort: Option<Vec<u8>>) -> Result<()> {
        self.post(k, "/v1/keypackages", &PublishKpReq { packages: packages.iter().map(|p| b64(p)).collect(), last_resort: last_resort.map(|p| b64(&p)) }).map(|_| ())
    }
    pub fn kp_count(&self, k: &SigningKey, device: &str) -> Result<u32> {
        let r = ok(self.t.request(Some(k), "GET", &format!("/v1/keypackages/{device}/count"), &[], T)?, "kpcount")?;
        Ok(String::from_utf8_lossy(&r.body).trim().parse().unwrap_or(0))
    }
    pub fn take_kp(&self, k: &SigningKey, device: &str) -> Result<Option<Vec<u8>>> {
        let r = ok(self.t.request(Some(k), "GET", &format!("/v1/keypackages/{device}"), &[], T)?, "kp")?;
        let v: KpResp = serde_json::from_slice(&r.body)?;
        Ok(match v.package { Some(p) => Some(unb64(&p)?), None => None })
    }
    pub fn acl(&self, k: &SigningKey, gid: &str, add: Vec<String>, remove: Vec<String>) -> Result<()> {
        self.post(k, &format!("/v1/groups/{gid}/acl"), &AclReq { add, remove }).map(|_| ())
    }
    pub fn post_entry(&self, k: &SigningKey, gid: &str, id: &str, data: &[u8]) -> Result<PostResp> {
        let r = self.post(k, &format!("/v1/groups/{gid}/messages"), &PostReq { id: id.into(), data: b64(data) })?;
        Ok(serde_json::from_slice(&r.body)?)
    }
    pub fn post_welcome(&self, k: &SigningKey, device: &str, id: &str, data: &[u8]) -> Result<()> {
        self.post(k, &format!("/v1/welcomes/{device}"), &PostReq { id: id.into(), data: b64(data) }).map(|_| ())
    }
    pub fn sync(&self, k: &SigningKey, req: &SyncReq) -> Result<SyncResp> {
        let body = serde_json::to_vec(req)?;
        let to = Duration::from_millis(req.wait_ms + 15_000);
        let r = ok(self.t.request(Some(k), "POST", "/v1/sync", &body, to)?, "sync")?;
        Ok(serde_json::from_slice(&r.body)?)
    }
    pub fn put_blob(&self, k: &SigningKey, id: &str, data: &[u8]) -> Result<()> {
        ok(self.t.request(Some(k), "PUT", &format!("/v1/blobs/{id}"), data, Duration::from_secs(300))?, "blob put").map(|_| ())
    }
    pub fn get_blob(&self, k: &SigningKey, id: &str) -> Result<Vec<u8>> {
        Ok(ok(self.t.request(Some(k), "GET", &format!("/v1/blobs/{id}"), &[], Duration::from_secs(300))?, "blob get")?.body)
    }
    pub fn rv_put(&self, rid: &str, slot: &str, data: &[u8]) -> Result<()> {
        ok(self.t.request(None, "PUT", &format!("/v1/rendezvous/{rid}/{slot}"), data, T)?, "rv put").map(|_| ())
    }
    pub fn rv_get(&self, rid: &str, slot: &str) -> Result<Option<Vec<u8>>> {
        let r = self.t.request(None, "GET", &format!("/v1/rendezvous/{rid}/{slot}"), &[], T)?;
        if r.status == 404 { return Ok(None); }
        Ok(Some(ok(r, "rv get")?.body))
    }
}
