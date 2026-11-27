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

pub struct HttpTransport { base: String, agent: ureq::Agent, bind: std::sync::Mutex<Option<(Option<String>, std::time::Instant)>> }
/// How long an answer to "does this relay bind requests to its host" is kept.
const BIND_TTL: Duration = Duration::from_secs(600);
impl HttpTransport {
    pub fn new(base: &str) -> Self {
        HttpTransport { base: base.trim_end_matches('/').to_string(), agent: ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(5)).build(), bind: std::sync::Mutex::new(None) }
    }
    /// The id to bind signed requests to, when this relay takes bound requests. `None`: sign the old way, as before. `fresh` asks the relay again.
    fn bind_id(&self, fresh: bool) -> Option<String> {
        if !fresh { if let Some((h, at)) = self.bind.lock().unwrap().as_ref() { if at.elapsed() < BIND_TTL { return h.clone(); } } }
        let r = self.send(None, "GET", "/v1/info", &[], Duration::from_secs(5), None).ok()?;
        let info: InfoResp = serde_json::from_slice(&r.body).ok()?;
        let id = (r.status == 200 && info.bind && info.relay_id.len() == 32 && info.relay_id.bytes().all(|c| c.is_ascii_hexdigit())).then_some(info.relay_id);
        *self.bind.lock().unwrap() = Some((id.clone(), std::time::Instant::now()));
        id
    }
    fn send(&self, key: Option<&SigningKey>, method: &str, path: &str, body: &[u8], timeout: Duration, relay_id: Option<&str>) -> Result<HttpResp> {
        let url = format!("{}{}", self.base, path);
        let mut r = self.agent.request(method, &url).timeout(timeout);
        if let Some(k) = key {
            r = match relay_id {
                Some(h) => r.set(AUTH2_HEADER, &sign_request_for(k, method, path, body, h)),
                None => r.set(AUTH_HEADER, &sign_request(k, method, path, body)),
            };
        }
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
impl HttpTransport { pub fn into_arc(self) -> std::sync::Arc<dyn Transport> { std::sync::Arc::new(self) } }
impl Transport for HttpTransport {
    fn request(&self, key: Option<&SigningKey>, method: &str, path: &str, body: &[u8], timeout: Duration) -> Result<HttpResp> {
        let id = key.and_then(|_| self.bind_id(false));
        let res = self.send(key, method, path, body, timeout, id.as_deref())?;
        if id.is_some() && res.status == 401 {
            // The relay's database was replaced (new id), or an older relay took its place: ask again, then say it again, bound or not.
            let now = self.bind_id(true);
            return self.send(key, method, path, body, timeout, now.as_deref());
        }
        Ok(res)
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
    /// Is this a Megatron relay at all?
    pub fn health(&self) -> Result<()> {
        let r = ok(self.t.request(None, "GET", "/health", &[], Duration::from_secs(5))?, "health")?;
        if r.body == b"ok" { Ok(()) } else { Err(anyhow!("that address answers, but it is not a Megatron relay")) }
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
    /// Send the relay's owner a problem report ([`crate::diag`]). An older relay without the route says 404.
    pub fn diag(&self, k: &SigningKey, report: &serde_json::Value) -> Result<()> {
        let body = serde_json::to_vec(report)?;
        let r = self.t.request(Some(k), "POST", "/v1/diag", &body, T)?;
        match r.status {
            200 => Ok(()),
            404 => Err(anyhow!("this relay does not take problem reports")),
            c => Err(anyhow!("problem report: relay said {c} {}", String::from_utf8_lossy(&r.body))),
        }
    }
    /// Who the relay's media server reports in these rooms' calls, as device ids. The relay only answers about
    /// rooms this account is a current member of; a relay without calls configured says 404.
    pub fn call_presence(&self, k: &SigningKey, gids: &[String]) -> Result<CallPresenceResp> {
        let body = serde_json::to_vec(&CallPresenceReq { gids: gids.to_vec() })?;
        let r = self.t.request(Some(k), "POST", "/v1/call-presence", &body, T)?;
        match r.status {
            200 => Ok(serde_json::from_slice(&r.body)?),
            404 => Err(anyhow!("this relay has no call server configured")),
            c => Err(anyhow!("call presence: relay said {c} {}", String::from_utf8_lossy(&r.body))),
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
    /// Post sealed live events (the relay holds them for seconds and never stores them). A relay without the channel says 404.
    pub fn live_post(&self, k: &SigningKey, gid: &str, blobs: &[Vec<u8>]) -> Result<LivePostResp> {
        let r = self.post(k, &format!("/v1/groups/{gid}/live"), &LivePostReq { events: blobs.iter().map(|b| b64(b)).collect() })?;
        Ok(serde_json::from_slice(&r.body)?)
    }
    pub fn live_poll(&self, k: &SigningKey, req: &LivePollReq) -> Result<LivePollResp> {
        let body = serde_json::to_vec(req)?;
        let to = Duration::from_millis(req.wait_ms + 15_000);
        let r = ok(self.t.request(Some(k), "POST", "/v1/live/poll", &body, to)?, "live poll")?;
        Ok(serde_json::from_slice(&r.body)?)
    }
    pub fn put_blob(&self, k: &SigningKey, id: &str, data: &[u8]) -> Result<()> {
        ok(self.t.request(Some(k), "PUT", &format!("/v1/blobs/{id}"), data, Duration::from_secs(300))?, "blob put").map(|_| ())
    }
    pub fn get_blob(&self, k: &SigningKey, id: &str) -> Result<Vec<u8>> {
        Ok(ok(self.t.request(Some(k), "GET", &format!("/v1/blobs/{id}"), &[], Duration::from_secs(300))?, "blob get")?.body)
    }
    // ---- large files: one chunk per request, so neither side ever holds the whole file ----
    pub fn large_begin(&self, k: &SigningKey, id: &str, req: &LargeBeginReq) -> Result<LargeInfoResp> {
        let r = self.post(k, &format!("/v1/large-files/{id}/begin"), req)?;
        Ok(serde_json::from_slice(&r.body)?)
    }
    pub fn large_info(&self, k: &SigningKey, id: &str) -> Result<LargeInfoResp> {
        let r = ok(self.t.request(Some(k), "GET", &format!("/v1/large-files/{id}/info"), &[], T)?, "large info")?;
        Ok(serde_json::from_slice(&r.body)?)
    }
    pub fn large_put_chunk(&self, k: &SigningKey, id: &str, index: u64, data: &[u8]) -> Result<()> {
        ok(self.t.request(Some(k), "PUT", &format!("/v1/large-files/{id}/chunks/{index}"), data, Duration::from_secs(300))?, "large chunk put").map(|_| ())
    }
    pub fn large_get_chunk(&self, k: &SigningKey, id: &str, index: u64) -> Result<Vec<u8>> {
        Ok(ok(self.t.request(Some(k), "GET", &format!("/v1/large-files/{id}/chunks/{index}"), &[], Duration::from_secs(300))?, "large chunk get")?.body)
    }
    pub fn large_finish(&self, k: &SigningKey, id: &str, sha256: &str) -> Result<LargeInfoResp> {
        let r = self.post(k, &format!("/v1/large-files/{id}/finish"), &LargeFinishReq { sha256: sha256.into() })?;
        Ok(serde_json::from_slice(&r.body)?)
    }
    pub fn large_delete(&self, k: &SigningKey, id: &str) -> Result<()> {
        ok(self.t.request(Some(k), "DELETE", &format!("/v1/large-files/{id}"), &[], T)?, "large delete").map(|_| ())
    }
    pub fn rv_put(&self, k: &SigningKey, rid: &str, slot: &str, data: &[u8]) -> Result<()> {
        ok(self.t.request(Some(k), "PUT", &format!("/v1/rendezvous/{rid}/{slot}"), data, T)?, "rv put").map(|_| ())
    }
    /// File a sealed password invite under its lookup id. A 409 means the id is taken: that password is in use.
    pub fn pw_put(&self, k: &SigningKey, lid: &str, body: &[u8]) -> Result<()> {
        ok(self.t.request(Some(k), "PUT", &format!("/v1/pwinvite/{lid}"), body, T)?, "password invite put").map(|_| ())
    }
    pub fn pw_delete(&self, k: &SigningKey, lid: &str) -> Result<()> {
        ok(self.t.request(Some(k), "DELETE", &format!("/v1/pwinvite/{lid}"), &[], T)?, "password invite delete").map(|_| ())
    }
    pub fn rv_get(&self, rid: &str, slot: &str) -> Result<Option<Vec<u8>>> {
        let r = self.t.request(None, "GET", &format!("/v1/rendezvous/{rid}/{slot}"), &[], T)?;
        if r.status == 404 { return Ok(None); }
        Ok(Some(ok(r, "rv get")?.body))
    }
}
