//! What can go wrong in a call, with the kinds and messages of call.js (`_explain`, `PASSING_ERRORS`, `LASTING_ERRORS`).
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Unsupported, Network, Removed, NotMember, NoServer, NoE2ee, Permission, NoDevice, Device, Mic, Camera, Screen, NoScreen, Busy, Play, Other }

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Unsupported => "unsupported", Kind::Network => "network", Kind::Removed => "removed", Kind::NotMember => "not-member",
            Kind::NoServer => "no-server", Kind::NoE2ee => "no-e2ee", Kind::Permission => "permission", Kind::NoDevice => "no-device",
            Kind::Device => "device", Kind::Mic => "mic", Kind::Camera => "camera", Kind::Screen => "screen", Kind::NoScreen => "no-screen", Kind::Busy => "busy",
            Kind::Play => "play", Kind::Other => "other",
        }
    }
    /// A notice about one thing that did not work: it fades after [`PASSING_FADE`].
    pub fn passing(self) -> bool { matches!(self, Kind::Screen | Kind::Camera | Kind::NoScreen | Kind::Busy | Kind::Device | Kind::Play) }
    /// The state of the call itself: it stays until the next call.
    pub fn lasting(self) -> bool { matches!(self, Kind::Network | Kind::Removed | Kind::Unsupported) }
}

pub const PASSING_FADE: Duration = Duration::from_secs(8);

#[derive(Clone, Debug, PartialEq)]
pub struct CallError { pub kind: Kind, pub msg: String }
impl CallError { pub fn new(kind: Kind, msg: impl Into<String>) -> CallError { CallError { kind, msg: msg.into() } } }
impl std::fmt::Display for CallError { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.msg) } }

fn has(m: &str, words: &[&str]) -> bool { words.iter().any(|w| m.contains(w)) }

/// The core or the SDK said something; what the person is told.
pub fn explain(m: &str) -> CallError {
    let l = m.to_lowercase();
    if l.contains("no call server configured") { return CallError::new(Kind::NoServer, "This relay has no call server configured. Ask whoever runs it to set up LiveKit (docs/DEPLOY.md)."); }
    if has(&l, &["not a current member", "not in this room", "no longer a member"]) { return CallError::new(Kind::NotMember, "You are not a current member of this room, so you cannot join its call."); }
    if has(&l, &["notallowed", "permission", "denied"]) { return CallError::new(Kind::Permission, "Microphone permission was denied. Allow microphone access for mega in your system settings, then try again."); }
    if has(&l, &["notfound", "requested device not found"]) { return CallError::new(Kind::NoDevice, "No microphone was found. Plug one in or pick another device in call settings."); }
    if has(&l, &["websocket", "could not establish", "signal", "timeout", "timed out", "econn", "connection refused", "failed to fetch", "networkerror", "connect", "dns", "tls", "io error"]) {
        return CallError::new(Kind::Network, format!("Could not reach the call server. Check your connection; if you run the server, check that its address and ports (TCP 443/7881, UDP 7882 or the TURN ports) are reachable. ({m})"));
    }
    CallError::new(Kind::Other, m)
}

/// `call_state` failing like this means this device is out of the room: the call ends at once (call.js `_startPoll`).
pub fn removed_from_room(m: &str) -> bool { has(m, &["no longer a member", "not joined", "out of sync"]) }

pub fn removed_notice(m: &str) -> CallError {
    if m.contains("no longer a member") { CallError::new(Kind::Removed, "You were removed from this room, so the call was ended on this device.") } else { CallError::new(Kind::Removed, m) }
}

/// A refused token is not reused.
pub fn token_refused(m: &str) -> bool { has(&m.to_lowercase(), &["401", "403", "unauthorized", "forbidden", "token is expired", "invalid token"]) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kinds_match_call_js() {
        assert_eq!(explain("you are no longer a member of this room").kind, Kind::NotMember);
        assert_eq!(explain("signal failure: ws error: Connection refused (os error 111)").kind, Kind::Network);
        assert_eq!(explain("this relay has no call server configured").kind, Kind::NoServer);
        assert_eq!(explain("something odd").kind, Kind::Other);
        assert!(Kind::Screen.passing() && !Kind::Screen.lasting() && Kind::Removed.lasting() && !Kind::Removed.passing());
    }
    #[test]
    fn removal_is_recognised_from_the_core_messages() {
        assert!(removed_from_room("you are no longer a member of this room"));
        assert!(removed_from_room("this room's keys are out of sync; waiting to be re-added"));
        assert!(!removed_from_room("locked"));
        assert_eq!(removed_notice("you are no longer a member of this room").kind, Kind::Removed);
    }
}
