//! One public port for everything, for hosts that have only one to give (Tailscale Funnel: docs/DEPLOY.md).
//! Whatever terminates TLS hands the plain stream here, and the first bytes say where it goes: `/rtc` is LiveKit's
//! signalling, any other HTTP is the relay, and a STUN message is TURN. Nothing is parsed past that, nothing is logged,
//! and only `/rtc` of LiveKit is reachable: its admin API is not.
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpListener, TcpStream}, sync::{oneshot, Semaphore}};

const MAX_FIRST_LINE: usize = 8 * 1024;
const FIRST_BYTES_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_CONNECTIONS: usize = 1024;

#[derive(Clone, Copy, Debug)]
pub struct Front { pub relay: SocketAddr, pub livekit: Option<SocketAddr>, pub turn: Option<SocketAddr> }

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Route { Relay, LiveKit, Turn, NeedMore, Reject }

/// A STUN message starts with two zero bits (RFC 8489), and TURN clients open with one. HTTP starts with a letter.
pub fn classify(b: &[u8]) -> Route {
    let Some(&first) = b.first() else { return Route::NeedMore };
    if first < 0x40 { return Route::Turn; }
    if !first.is_ascii_uppercase() { return Route::Reject; }
    let Some(end) = b.iter().position(|c| *c == b'\n') else { return if b.len() >= MAX_FIRST_LINE { Route::Reject } else { Route::NeedMore } };
    let Ok(line) = std::str::from_utf8(&b[..end]) else { return Route::Reject };
    let mut parts = line.trim_end().split(' ');
    let (Some(_), Some(path), Some(version)) = (parts.next(), parts.next(), parts.next()) else { return Route::Reject };
    if !version.starts_with("HTTP/1.") { return Route::Reject; }
    let path = path.split(['?', '#']).next().unwrap_or("");
    if path == "/rtc" || path.starts_with("/rtc/") { Route::LiveKit } else { Route::Relay }
}

async fn one(mut c: TcpStream, f: Front) -> std::io::Result<()> {
    let mut buf = Vec::with_capacity(1024);
    let route = tokio::time::timeout(FIRST_BYTES_TIMEOUT, async {
        loop {
            match classify(&buf) { Route::NeedMore => {} r => return Ok::<_, std::io::Error>(r) }
            if c.read_buf(&mut buf).await? == 0 { return Ok(Route::Reject); }
        }
    }).await.unwrap_or(Ok(Route::Reject))?;
    let to = match route { Route::Relay => Some(f.relay), Route::LiveKit => f.livekit, Route::Turn => f.turn, _ => None };
    let Some(to) = to else { return Ok(()) };
    let mut up = TcpStream::connect(to).await?;
    let _ = up.set_nodelay(true);
    let _ = c.set_nodelay(true);
    up.write_all(&buf).await?;
    tokio::io::copy_bidirectional(&mut c, &mut up).await?;
    Ok(())
}

pub struct FrontHandle { pub addr: SocketAddr, stop: Option<oneshot::Sender<()>> }
impl FrontHandle { pub fn stop(mut self) { if let Some(s) = self.stop.take() { let _ = s.send(()); } } }

pub async fn start_front(bind: SocketAddr, f: Front) -> std::io::Result<FrontHandle> {
    let l = TcpListener::bind(bind).await?;
    let addr = l.local_addr()?;
    let (tx, mut rx) = oneshot::channel();
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut rx => break,
                got = l.accept() => {
                    let Ok((c, _)) = got else { tokio::time::sleep(Duration::from_millis(50)).await; continue };
                    let Ok(slot) = slots.clone().try_acquire_owned() else { continue };
                    tokio::spawn(async move { let _ = one(c, f).await; drop(slot); });
                }
            }
        }
    });
    Ok(FrontHandle { addr, stop: Some(tx) })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_first_bytes_pick_the_route() {
        assert_eq!(classify(b""), Route::NeedMore);
        assert_eq!(classify(b"GET /rt"), Route::NeedMore);
        assert_eq!(classify(b"GET /rtc/v1?access_token=x HTTP/1.1\r\n"), Route::LiveKit);
        assert_eq!(classify(b"GET /rtc?x=1 HTTP/1.1\r\n"), Route::LiveKit);
        assert_eq!(classify(b"GET /rtcx HTTP/1.1\r\n"), Route::Relay);
        assert_eq!(classify(b"POST /v1/sync HTTP/1.1\r\n"), Route::Relay);
        assert_eq!(classify(b"POST /twirp/livekit.RoomService/ListRooms HTTP/1.1\r\n"), Route::Relay, "LiveKit's admin API stays out of reach");
        assert_eq!(classify(&[0x00, 0x03, 0x00, 0x00, 0x21, 0x12, 0xa4, 0x42]), Route::Turn);
        assert_eq!(classify(b"\x16\x03\x01"), Route::Turn, "whatever else starts low goes to TURN, which drops it");
        assert_eq!(classify(b"PRI * HTTP/2.0\r\n"), Route::Reject);
        assert_eq!(classify(b"garbage"), Route::Reject);
        assert_eq!(classify(&vec![b'A'; MAX_FIRST_LINE]), Route::Reject);
    }

    async fn echo(tag: &'static [u8]) -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let a = l.local_addr().unwrap();
        tokio::spawn(async move { loop {
            let (mut c, _) = l.accept().await.unwrap();
            tokio::spawn(async move { let mut b = [0u8; 256]; let n = c.read(&mut b).await.unwrap(); c.write_all(tag).await.unwrap(); c.write_all(&b[..n]).await.unwrap(); });
        } });
        a
    }
    async fn ask(a: SocketAddr, what: &[u8]) -> Vec<u8> {
        let mut c = TcpStream::connect(a).await.unwrap();
        c.write_all(what).await.unwrap();
        let mut out = vec![];
        let _ = tokio::time::timeout(Duration::from_secs(2), c.read_to_end(&mut out)).await;
        out
    }
    #[tokio::test]
    async fn streams_reach_the_right_place_with_nothing_lost() {
        let f = Front { relay: echo(b"relay:").await, livekit: Some(echo(b"lk:").await), turn: Some(echo(b"turn:").await) };
        let h = start_front("127.0.0.1:0".parse().unwrap(), f).await.unwrap();
        assert_eq!(ask(h.addr, b"GET /health HTTP/1.1\r\n\r\n").await, b"relay:GET /health HTTP/1.1\r\n\r\n");
        assert_eq!(ask(h.addr, b"GET /rtc/v1 HTTP/1.1\r\n\r\n").await, b"lk:GET /rtc/v1 HTTP/1.1\r\n\r\n");
        assert_eq!(ask(h.addr, &[0, 3, 0, 0]).await, [b"turn:".as_slice(), &[0, 3, 0, 0]].concat());
        assert!(ask(h.addr, b"\xff\xff").await.is_empty());
        h.stop();
    }
    #[tokio::test]
    async fn no_calls_configured_means_only_the_relay() {
        let f = Front { relay: echo(b"relay:").await, livekit: None, turn: None };
        let h = start_front("127.0.0.1:0".parse().unwrap(), f).await.unwrap();
        assert!(ask(h.addr, b"GET /rtc/v1 HTTP/1.1\r\n\r\n").await.is_empty());
        h.stop();
    }
}
