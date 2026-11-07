//! Runs the engine for as long as a call asks for it, and not a moment longer: with no call, no key is looked at.
//! A call keeps asking (`wait`), and that doubles as the heartbeat in both directions: if the app stops answering, the call
//! page lets go; if the page stops asking, the helper stops.
use crate::backend::{self, Keys};
use crate::{Engine, Settings};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(10);
const IDLE_POLL: Duration = Duration::from_millis(250);
const PROCESS_SCAN: Duration = Duration::from_secs(2);
const RETRY_AFTER_ERROR: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status { pub seq: u64, pub muted: bool, pub game: Option<String>, pub running: bool, pub error: String }

type Open = dyn Fn(&Settings) -> Result<Box<dyn Keys>, String> + Send + Sync;
type Scan = dyn Fn() -> Vec<String> + Send + Sync;

struct Inner { engine: Engine, settings: Settings, status: Status, lease_until: Instant, running: bool, reopen: bool, not_before: Instant }
pub struct Service { s: Arc<(Mutex<Inner>, Condvar)>, open: Arc<Open>, scan: Arc<Scan>, lease: Duration }

fn publish(g: &mut Inner, cv: &Condvar) {
    let (muted, game) = (g.engine.muted(), g.engine.because());
    if muted != g.status.muted || (muted && game != g.status.game) { g.status.muted = muted; g.status.game = game; g.status.seq += 1; cv.notify_all(); }
}

impl Service {
    pub fn new() -> Service { Service::with(Arc::new(backend::open), Arc::new(backend::processes), Duration::from_secs(12)) }
    pub fn with(open: Arc<Open>, scan: Arc<Scan>, lease: Duration) -> Service {
        let now = Instant::now();
        Service { s: Arc::new((Mutex::new(Inner { engine: Engine::new(), settings: Settings::default(), status: Status::default(), lease_until: now, running: false, reopen: false, not_before: now }), Condvar::new())), open, scan, lease }
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> { self.s.0.lock().unwrap_or_else(|p| p.into_inner()) }

    pub fn set_settings(&self, s: Settings) {
        let mut g = self.lock();
        if g.settings == s { return; }
        g.engine.set_rules(s.rules.clone());
        g.settings = s;
        g.reopen = true;
        g.not_before = Instant::now();
        publish(&mut g, &self.s.1);
    }
    pub fn status(&self) -> Status { self.lock().status.clone() }
    /// The person pressed Unmute in the call.
    pub fn user_unmute(&self) { let mut g = self.lock(); g.engine.user_unmute(); publish(&mut g, &self.s.1); }

    /// What a call asks: keep the helper going, and tell me when the answer differs from `since` (or at `timeout`).
    pub fn wait(&self, since: u64, timeout: Duration) -> Status {
        let mut g = self.lock();
        g.lease_until = Instant::now() + self.lease;
        if !g.running && !g.settings.rules.is_empty() && Instant::now() >= g.not_before { g.running = true; self.start(); }
        let (g, _) = self.s.1.wait_timeout_while(g, timeout, |g| g.status.seq == since).unwrap_or_else(|p| p.into_inner());
        g.status.clone()
    }

    fn start(&self) {
        let (s, open, scan) = (self.s.clone(), self.open.clone(), self.scan.clone());
        std::thread::spawn(move || {
            // Whatever ends this thread, a panic included, the mic goes back to the person.
            struct Guard(Arc<(Mutex<Inner>, Condvar)>);
            impl Drop for Guard {
                fn drop(&mut self) {
                    let mut g = self.0.0.lock().unwrap_or_else(|p| p.into_inner());
                    g.engine.helper_lost();
                    g.running = false;
                    g.status.running = false;
                    publish(&mut g, &self.0.1);
                    self.0.1.notify_all();
                }
            }
            let _guard = Guard(s.clone());
            let settings = { s.0.lock().unwrap_or_else(|p| p.into_inner()).settings.clone() };
            let fail = |why: String| { let mut g = s.0.lock().unwrap_or_else(|p| p.into_inner()); g.status.error = why; g.status.seq += 1; g.not_before = Instant::now() + RETRY_AFTER_ERROR; };
            let mut keys = match open(&settings) { Ok(k) => k, Err(e) => { fail(e); return; } };
            { let mut g = s.0.lock().unwrap_or_else(|p| p.into_inner()); g.engine.helper_back(); g.reopen = false; g.status.running = true; g.status.error.clear(); g.status.seq += 1; s.1.notify_all(); }
            let start = Instant::now();
            let mut last_scan: Option<Instant> = None;
            loop {
                if last_scan.is_none_or(|t| t.elapsed() >= PROCESS_SCAN) {
                    let list = scan();
                    last_scan = Some(Instant::now());
                    let mut g = s.0.lock().unwrap_or_else(|p| p.into_inner());
                    g.engine.processes(list);
                    publish(&mut g, &s.1);
                }
                let wanted = {
                    let g = s.0.lock().unwrap_or_else(|p| p.into_inner());
                    if Instant::now() >= g.lease_until || g.reopen { return; }
                    g.engine.wanted()
                };
                let idle = wanted.is_empty();
                let down = if idle { Ok(vec![]) } else { keys.down(&wanted) };
                let down = match down { Ok(d) => d, Err(e) => { fail(e); return; } };
                let now = start.elapsed().as_millis() as u64;
                {
                    let mut g = s.0.lock().unwrap_or_else(|p| p.into_inner());
                    g.engine.keys(&down, now);
                    g.engine.tick(now);
                    publish(&mut g, &s.1);
                }
                std::thread::sleep(if idle { IDLE_POLL } else { POLL });
            }
        });
    }
}
impl Default for Service { fn default() -> Self { Self::new() } }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Mode, Rule};
    use std::sync::atomic::{AtomicU8, Ordering};

    /// 0 up, 1 down, 2 error, 3 panic.
    struct Fake(Arc<AtomicU8>);
    impl Keys for Fake {
        fn down(&mut self, wanted: &[String]) -> Result<Vec<String>, String> {
            match self.0.load(Ordering::SeqCst) { 0 => Ok(vec![]), 1 => Ok(wanted.to_vec()), 2 => Err("helper gone".into()), _ => panic!("helper crashed") }
        }
    }
    fn world(names: Arc<Mutex<Vec<String>>>) -> (Service, Arc<AtomicU8>) {
        let key = Arc::new(AtomicU8::new(0));
        let k2 = key.clone();
        let s = Service::with(Arc::new(move |_| Ok(Box::new(Fake(k2.clone())) as Box<dyn Keys>)), Arc::new(move || names.lock().unwrap().clone()), Duration::from_millis(400));
        s.set_settings(Settings { rules: vec![Rule { process: Some("game".into()), key: "KeyV".into(), mode: Mode::Hold }], use_portal: false });
        (s, key)
    }
    fn until(s: &Service, want: bool, ms: u64) -> Status {
        let end = Instant::now() + Duration::from_millis(ms);
        loop {
            let st = s.wait(s.status().seq, Duration::from_millis(50));
            if st.muted == want || Instant::now() >= end { return st; }
        }
    }
    #[test]
    fn a_held_key_mutes_and_comes_back_after_the_delay() {
        let (s, key) = world(Arc::new(Mutex::new(vec!["game".into()])));
        assert!(!until(&s, true, 100).muted);
        key.store(1, Ordering::SeqCst);
        let st = until(&s, true, 500);
        assert!(st.muted);
        assert_eq!(st.game.as_deref(), Some("game"));
        key.store(0, Ordering::SeqCst);
        let t = Instant::now();
        assert!(!until(&s, false, 800).muted);
        assert!(t.elapsed() >= Duration::from_millis(crate::RELEASE_MS - 20), "{:?}", t.elapsed());
    }
    #[test]
    fn a_helper_that_fails_gives_the_mic_back_and_says_why() {
        let (s, key) = world(Arc::new(Mutex::new(vec!["game".into()])));
        key.store(1, Ordering::SeqCst);
        assert!(until(&s, true, 500).muted);
        key.store(2, Ordering::SeqCst);
        let st = until(&s, false, 500);
        assert!(!st.muted);
        assert_eq!(st.error, "helper gone");
    }
    #[test]
    fn a_helper_that_panics_gives_the_mic_back() {
        let (s, key) = world(Arc::new(Mutex::new(vec!["game".into()])));
        key.store(1, Ordering::SeqCst);
        assert!(until(&s, true, 500).muted);
        key.store(3, Ordering::SeqCst);
        assert!(!until(&s, false, 500).muted);
    }
    #[test]
    fn the_game_closing_gives_the_mic_back() {
        let names = Arc::new(Mutex::new(vec!["game".to_string()]));
        let (s, key) = world(names.clone());
        key.store(1, Ordering::SeqCst);
        assert!(until(&s, true, 500).muted);
        names.lock().unwrap().clear();
        assert!(!until(&s, false, 3000).muted);
    }
    #[test]
    fn a_call_ending_stops_the_helper() {
        let (s, _) = world(Arc::new(Mutex::new(vec!["game".into()])));
        s.wait(0, Duration::from_millis(100));
        assert!(s.status().running);
        std::thread::sleep(Duration::from_millis(700));
        assert!(!s.status().running, "no call, no looking at keys");
    }
    #[test]
    fn with_no_rules_nothing_runs() {
        let s = Service::with(Arc::new(|_| panic!("must not open")), Arc::new(Vec::new), Duration::from_millis(100));
        assert!(!s.wait(0, Duration::from_millis(30)).running);
    }
}
