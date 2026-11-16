//! Which ICE servers a call may use (patch L1, CALLS-PLAN.md section 3). The same rule as `fixIce` in call.js: Google's
//! and Twilio's public STUN servers are never contacted (they would learn who is calling), and TURN over TLS is moved
//! from the port 443 LiveKit tells everyone to the port the relay says it really listens on.
use livekit::rtc_engine::IceServersFilter;
use livekit::webrtc::prelude::IceServer;
use std::sync::Arc;

/// `stun:` or `stuns:` on a host under google.com or twilio.com (case does not matter), like call.js's
/// `/^stuns?:.*(google|twilio)\.com/i`.
fn banned(url: &str) -> bool {
    let u = url.to_ascii_lowercase();
    (u.starts_with("stun:") || u.starts_with("stuns:")) && (u.contains("google.com") || u.contains("twilio.com"))
}

/// `turns:host:443...` becomes `turns:host:<port>...`, like call.js's `/^(turns:[^:?]+):443\b/i`.
fn move_turns(url: &str, port: u16) -> String {
    if !url.get(..6).is_some_and(|p| p.eq_ignore_ascii_case("turns:")) { return url.to_string(); }
    let rest = &url[6..];
    let host_end = rest.find([':', '?']).unwrap_or(rest.len());
    let after = &rest[host_end..];
    if host_end == 0 || !after.starts_with(":443") { return url.to_string(); }
    let tail = &after[4..];
    // `\b` after 443: the port must end there (not :4430)
    if tail.chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') { return url.to_string(); }
    format!("{}{}:{}{}", &url[..6], &rest[..host_end], port, tail)
}

pub fn fix_ice(servers: Vec<IceServer>, turn_port: Option<u16>) -> Vec<IceServer> {
    servers.into_iter().filter_map(|mut s| {
        s.urls = s.urls.into_iter().filter(|u| !banned(u)).map(|u| match turn_port { Some(p) => move_turns(&u, p), None => u }).collect();
        (!s.urls.is_empty()).then_some(s)
    }).collect()
}

/// The hook handed to `RoomOptions::ice_servers_filter`. `seen` (tests only) receives the server's list as it arrived,
/// with credentials left out.
pub fn filter(turn_port: Option<u16>, seen: Option<Arc<dyn Fn(Vec<String>) + Send + Sync>>) -> IceServersFilter {
    IceServersFilter(Arc::new(move |servers: Vec<IceServer>| {
        if let Some(f) = &seen { f(servers.iter().flat_map(|s| s.urls.clone()).collect()); }
        fix_ice(servers, turn_port)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn s(urls: &[&str]) -> IceServer { IceServer { urls: urls.iter().map(|u| u.to_string()).collect(), username: "u".into(), password: "p".into() } }

    #[test]
    fn google_and_twilio_stun_are_removed_and_empty_entries_go() {
        let out = fix_ice(vec![s(&["stun:stun.l.google.com:19302", "stun:global.stun.twilio.com:3478"]), s(&["STUNS:Stun1.L.Google.com:443", "stun:stun.example.org:3478"])], None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].urls, vec!["stun:stun.example.org:3478"]);
        assert_eq!(out[0].username, "u");
    }
    #[test]
    fn turn_over_tls_moves_off_443_only_when_told() {
        let list = || vec![s(&["turns:turn.example.org:443?transport=tcp", "turn:turn.example.org:443?transport=udp", "turns:turn.example.org:4430", "turns:other:443"])];
        assert_eq!(fix_ice(list(), Some(5349))[0].urls, vec!["turns:turn.example.org:5349?transport=tcp", "turn:turn.example.org:443?transport=udp", "turns:turn.example.org:4430", "turns:other:5349"]);
        assert_eq!(fix_ice(list(), None)[0].urls, list()[0].urls);
    }
    #[test]
    fn a_turn_server_on_google_is_not_a_stun_server() {
        assert_eq!(fix_ice(vec![s(&["turn:turn.google.com:3478"])], None)[0].urls, vec!["turn:turn.google.com:3478"]);
    }
}
