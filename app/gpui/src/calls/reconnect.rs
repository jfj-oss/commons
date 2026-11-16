//! When to try again, with call.js's constants (CALLS-PLAN.md section 8, "Reconnects").
//!
//! The SDK may try one quick resume. If it is not back within [`RESUME_GRACE`], or the session ends, or the media path
//! stays down for [`MEDIA_DOWN_POLLS`] polls, the engine closes the room and builds a fresh one with a new key provider.
use std::time::Duration;

/// Before telling the person that a first join failed.
pub const JOIN_RETRY: [Duration; 4] = [Duration::from_millis(700), Duration::from_millis(1500), Duration::from_millis(3000), Duration::from_millis(5000)];
/// Between attempts to rebuild a call that was lost.
pub const REJOIN: [Duration; 6] = [Duration::from_millis(400), Duration::from_millis(400), Duration::from_millis(800), Duration::from_millis(1000), Duration::from_millis(1500), Duration::from_millis(2000)];
pub const REJOIN_SLOW_AFTER: Duration = Duration::from_secs(60);
pub const REJOIN_SLOW: Duration = Duration::from_secs(5);
pub const REJOIN_GIVE_UP: Duration = Duration::from_secs(600);
/// How long the SDK's own resume may take before the call is rebuilt.
pub const RESUME_GRACE: Duration = Duration::from_secs(3);
/// About 10 s of polls with no working media path.
pub const MEDIA_DOWN_POLLS: u32 = 7;
pub const POLL: Duration = Duration::from_millis(1500);
pub const RPC_TIMEOUT: Duration = Duration::from_secs(15);
/// A rejoin attempt starts with a tighter limit, back to the full one if tight attempts keep failing (a slow link).
pub const REJOIN_ATTEMPT_LIMIT: Duration = Duration::from_secs(10);
pub const REJOIN_TIGHT_ATTEMPTS: usize = 3;
pub const FIRST_JOIN_LIMIT: Duration = Duration::from_secs(20);
/// A token is reused for a retry only with at least this long left.
pub const TOKEN_MARGIN_SECS: u64 = 60;

/// The wait after the `i`-th failed rebuild attempt (0-based), `since` the call was lost.
pub fn rejoin_delay(i: usize, since: Duration) -> Duration {
    if since > REJOIN_SLOW_AFTER { REJOIN_SLOW } else { REJOIN[i.min(REJOIN.len() - 1)] }
}

pub fn attempt_limit(i: usize) -> Duration { if i < REJOIN_TIGHT_ATTEMPTS { REJOIN_ATTEMPT_LIMIT } else { FIRST_JOIN_LIMIT } }

/// `expires` in seconds since the epoch (0: unknown, then it is reused).
pub fn token_reusable(expires: u64, now: u64) -> bool { expires == 0 || expires.saturating_sub(now) > TOKEN_MARGIN_SECS }

/// Counts polls, not seconds, so a machine that slept does not look like a long outage the moment it wakes.
#[derive(Default, Debug)]
pub struct MediaWatch { down: u32 }
impl MediaWatch {
    /// True when the call should be rebuilt now.
    pub fn poll(&mut self, connected: bool) -> bool {
        self.down = if connected { 0 } else { self.down + 1 };
        if self.down >= MEDIA_DOWN_POLLS { self.down = 0; true } else { false }
    }
    pub fn reset(&mut self) { self.down = 0; }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejoin_waits_follow_call_js_then_slow_down() {
        let w: Vec<u64> = (0..8).map(|i| rejoin_delay(i, Duration::from_secs(5)).as_millis() as u64).collect();
        assert_eq!(w, vec![400, 400, 800, 1000, 1500, 2000, 2000, 2000]);
        assert_eq!(rejoin_delay(2, Duration::from_secs(61)), REJOIN_SLOW);
    }
    #[test]
    fn tokens_are_reused_only_with_a_minute_left() {
        assert!(token_reusable(1000, 900));
        assert!(!token_reusable(1000, 950));
        assert!(token_reusable(0, 5));
    }
    #[test]
    fn seven_bad_polls_rebuild_and_one_good_poll_resets() {
        let mut m = MediaWatch::default();
        for _ in 0..6 { assert!(!m.poll(false)); }
        assert!(!m.poll(true));
        for _ in 0..6 { assert!(!m.poll(false)); }
        assert!(m.poll(false));
    }
}
