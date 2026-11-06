//! The only network surface of the client. A trait so tests can inject faults (duplicates, reordering, drops).
use anyhow::{anyhow, Result};
use commons_proto::*;
use ed25519_dalek::SigningKey;
use std::time::Duration;

pub struct HttpResp { pub status: u16, pub body: Vec<u8> }

/// The relay answered, and the answer was a refusal.
#[derive(Debug, thiserror::Error)]
#[error("{what}: relay said {status} {body}")]
pub struct RelayError { pub what: String, pub status: u16, pub body: String }

/// A response larger than anything we are willing to hold. It is reported, never cut short into something that
/// looks like a damaged message.
#[derive(Debug, thiserror::Error)]
#[error("the relay sent a response larger than {0} bytes")]
pub struct TooLarge(pub u64);

/// A reply we could not use: too big, cut short, or not what was expected. Worth asking for less.
pub fn is_unreadable(e: &anyhow::Error) -> bool { e.is::<serde_json::Error>() || e.is::<TooLarge>() }
/// The relay refused with this status.
pub fn relay_status(e: &anyhow::Error) -> Option<u16> { e.downcast_ref::<RelayError>().map(|r| r.status) }
pub fn relay_says(e: &anyhow::Error, status: u16, what: &str) -> bool {
    e.downcast_ref::<RelayError>().is_some_and(|r| r.status == status && r.body.contains(what))
}

pub trait Transport: Send + Sync {
    /// `key` signs the request when Some.
    fn request(&self, key: Option<&SigningKey>, method: &str, path: &str, body: &[u8], timeout: Duration) -> Result<HttpResp>;
}

pub struct HttpTransport { base: String, agent: ureq::Agent }
impl HttpTransport {
    pub fn new(base: &str) -> Self {
        HttpTransport { base: base.trim_end_matches('/').to_string(), agent: ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(5)).build() }
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
                let cap = MAX_BLOB_BYTES as u64 + 4096;
                let mut b = Vec::new();
                resp.into_reader().take(cap + 1).read_to_end(&mut b)?;
                if b.len() as u64 > cap { return Err(TooLarge(cap).into()); }
                Ok(HttpResp { status, body: b })
            }
            Err(ureq::Error::Status(code, resp)) => {
                let mut b = Vec::new();
                let _ = resp.into_reader().take(4096).read_to_end(&mut b);
                Ok(HttpResp { status: code, body: b })
            }
            Err(e) => Err(anyhow!("network: {e}")),
        }
    }
}
use std::io::Read as _;

/// Typed relay calls on top of a Transport.
#[derive(Clone)]
pub struct Api { pub t: std::sync::Arc<dyn Transport> }
const T: Duration = Duration::from_secs(15);
fn ok(r: HttpResp, what: &str) -> Result<HttpResp> {
    if r.status / 100 == 2 { Ok(r) } else { Err(RelayError { what: what.into(), status: r.status, body: String::from_utf8_lossy(&r.body).into_owned() }.into()) }
}
impl Api {
    fn post<B: serde::Serialize>(&self, k: &SigningKey, path: &str, b: &B) -> Result<HttpResp> {
        let body = serde_json::to_vec(b)?;
        ok(self.t.request(Some(k), "POST", path, &body, T)?, path)
    }
    pub fn register(&self, k: &SigningKey, cert: &DeviceCert, invite: Option<String>) -> Result<()> { self.post(k, "/v1/devices", &RegisterReq { cert: cert.clone(), invite }).map(|_| ()) }
    /// Is this a Commons relay at all?
    pub fn health(&self) -> Result<()> {
        let r = ok(self.t.request(None, "GET", "/health", &[], Duration::from_secs(5))?, "health")?;
        if r.body == b"ok" { Ok(()) } else { Err(anyhow!("that address answers, but it is not a Commons relay")) }
    }
    /// Does this relay need an invite secret, and (given one) is it the right one?
    pub fn info(&self, invite: Option<&str>) -> Result<InfoResp> {
        let r = match invite {
            Some(i) => self.t.request(None, "POST", "/v1/info", &serde_json::to_vec(&InfoReq { invite: i.into() })?, Duration::from_secs(5))?,
            None => self.t.request(None, "GET", "/v1/info", &[], Duration::from_secs(5))?,
        };
        Ok(serde_json::from_slice(&ok(r, "info")?.body)?)
    }
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
    pub fn acl(&self, k: &SigningKey, gid: &str, req: &AclReq) -> Result<()> {
        self.post(k, &format!("/v1/groups/{gid}/acl"), req).map(|_| ())
    }
    /// Ask the relay for a LiveKit join token for this group's call.
    pub fn call_token(&self, k: &SigningKey, gid: &str) -> Result<CallTokenResp> {
        let r = self.t.request(Some(k), "POST", &format!("/v1/groups/{gid}/call-token"), b"{}", T)?;
        match r.status {
            200 => Ok(serde_json::from_slice(&r.body)?),
            404 => Err(anyhow!("this relay has no call server configured")),
            403 => Err(anyhow!("the relay says you are not a current member of this room")),
            c => Err(anyhow!("call token: relay said {c} {}", String::from_utf8_lossy(&r.body))),
        }
    }
    pub fn acl_get(&self, k: &SigningKey, gid: &str) -> Result<AclResp> {
        let r = ok(self.t.request(Some(k), "GET", &format!("/v1/groups/{gid}/acl"), &[], T)?, "acl")?;
        Ok(serde_json::from_slice(&r.body)?)
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
    pub fn rv_put(&self, k: &SigningKey, rid: &str, slot: &str, data: &[u8]) -> Result<()> {
        ok(self.t.request(Some(k), "PUT", &format!("/v1/rendezvous/{rid}/{slot}"), data, T)?, "rv put").map(|_| ())
    }
    pub fn rv_get(&self, rid: &str, slot: &str) -> Result<Option<Vec<u8>>> {
        let r = self.t.request(None, "GET", &format!("/v1/rendezvous/{rid}/{slot}"), &[], T)?;
        if r.status == 404 { return Ok(None); }
        Ok(Some(ok(r, "rv get")?.body))
    }
}
