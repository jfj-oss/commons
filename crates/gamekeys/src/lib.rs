//! Per-game push-to-mute. While you hold your game's voice key, your mic in the Commons call goes quiet, so friends do not
//! hear what you say to your game team.
//!
//! Three layers. `Engine` is the decision, with no clock and no operating system in it, so every rule can be tested.
//! `Service` runs it in a thread while a call wants it. `backend` is the operating-system part: it can only look. It never
//! grabs a key, never injects anything, never opens a game's process. It reads the state of the few keys you configured,
//! and the process list by name.
pub mod backend;
#[cfg(target_os = "linux")]
pub mod portal;
pub mod keys;
pub mod service;
pub use service::{Service, Status};

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};

/// After a key comes up the mic stays off this long, so the end of a word does not leak.
pub const RELEASE_MS: u64 = 200;
pub const MAX_RULES: usize = 64;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode { #[default] Hold, Toggle }

/// One game, or with `process: None` the "any app" key. `key` is a `KeyboardEvent.code` (`KeyV`, `F13`) or `Mouse4`/`Mouse5`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    #[serde(default)]
    pub process: Option<String>,
    pub key: String,
    #[serde(default)]
    pub mode: Mode,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(default)]
pub struct Settings {
    pub rules: Vec<Rule>,
    /// Linux: bind the keys as system shortcuts through the desktop portal. Off by default, because a compositor keeps such a
    /// key for itself, and the game may not get it (docs/GAME-KEYS.md).
    pub use_portal: bool,
}

impl Settings {
    pub fn check(&self) -> Result<(), String> {
        if self.rules.len() > MAX_RULES { return Err(format!("at most {MAX_RULES} games")); }
        let mut seen = HashSet::new();
        for r in &self.rules {
            if !keys::known(&r.key) { return Err(format!("{} cannot be used as a key", r.key)); }
            if let Some(p) = &r.process {
                if p.trim().is_empty() || p.len() > 255 || p.chars().any(char::is_control) { return Err("that is not a program name".into()); }
            }
            if !seen.insert(r.process.as_deref().map(norm)) { return Err(match &r.process { Some(p) => format!("{p} is on the list twice"), None => "there is only one \"any app\" key".into() }); }
        }
        Ok(())
    }
}

/// One spelling for a program name: Windows and Wine do not care about case, and the same game can show up with or without `.exe`.
pub fn norm(p: &str) -> String {
    let p = p.trim().to_lowercase();
    p.strip_suffix(".exe").map(str::to_string).unwrap_or(p)
}

/// Decides whether the mic is held down by a game key. Times are milliseconds on any clock that only goes forward.
#[derive(Default, Debug)]
pub struct Engine {
    rules: Vec<Rule>,
    running: HashSet<String>,
    down: BTreeSet<String>,
    /// Rules switched on with a toggle key, by their position in `rules`.
    toggled: BTreeSet<usize>,
    /// The mic stays off until this time, after the last reason went away.
    release_at: Option<u64>,
    lost: bool,
}

impl Engine {
    pub fn new() -> Engine { Engine::default() }

    /// New rules start clean: a toggle does not survive them.
    pub fn set_rules(&mut self, rules: Vec<Rule>) {
        if rules != self.rules { self.toggled.clear(); self.release_at = None; }
        self.rules = rules;
    }
    fn active(&self, r: &Rule) -> bool {
        match &r.process { None => true, Some(p) => self.running.contains(&norm(p)) }
    }
    fn reason(&self) -> bool {
        if self.lost { return false; }
        self.rules.iter().enumerate().any(|(i, r)| self.active(r) && match r.mode { Mode::Hold => self.down.contains(&r.key), Mode::Toggle => self.toggled.contains(&i) })
    }
    pub fn muted(&self) -> bool { self.reason() || (!self.lost && self.release_at.is_some()) }
    /// Keys the helper has to look at now, and nothing else: an unconfigured key is never read at all.
    pub fn wanted(&self) -> Vec<String> {
        let mut k: Vec<String> = self.rules.iter().filter(|r| self.active(r)).map(|r| r.key.clone()).collect();
        k.sort(); k.dedup(); k
    }
    /// The rule that is holding the mic down, for the panel. None for "any app" or nothing.
    pub fn because(&self) -> Option<String> {
        self.rules.iter().enumerate().find(|(i, r)| self.active(r) && match r.mode { Mode::Hold => self.down.contains(&r.key), Mode::Toggle => self.toggled.contains(i) }).and_then(|(_, r)| r.process.clone())
    }

    /// Which programs run now. A game that closed takes its keys with it at once: there is no word to protect any more.
    pub fn processes(&mut self, running: impl IntoIterator<Item = String>) {
        let was = self.reason();
        self.running = running.into_iter().map(|p| norm(&p)).collect();
        let gone: Vec<usize> = self.toggled.iter().copied().filter(|i| !self.rules.get(*i).is_some_and(|r| self.active(r))).collect();
        for i in gone { self.toggled.remove(&i); }
        if was && !self.reason() { self.release_at = None; }
        if self.rules.iter().all(|r| r.process.is_some() && !self.active(r)) { self.release_at = None; }
    }

    /// The full set of keys that are down now, as the helper sees it.
    pub fn keys(&mut self, down: &[String], now: u64) {
        if self.lost { return; }
        let was = self.reason();
        let down: BTreeSet<String> = down.iter().cloned().collect();
        for (i, r) in self.rules.iter().enumerate() {
            if r.mode == Mode::Toggle && self.active(r) && down.contains(&r.key) && !self.down.contains(&r.key) && !self.toggled.remove(&i) { self.toggled.insert(i); }
        }
        self.down = down;
        let is = self.reason();
        if is { self.release_at = None; } else if was { self.release_at = Some(now + RELEASE_MS); }
    }
    pub fn tick(&mut self, now: u64) {
        if self.release_at.is_some_and(|t| now >= t) { self.release_at = None; }
    }
    /// The person pressed Unmute themselves: that beats a toggle. A held key is still held, and it wins for as long as it is.
    pub fn user_unmute(&mut self) {
        self.toggled.clear();
        if !self.reason() { self.release_at = None; }
    }
    /// The helper is gone, or stopped: nothing can be known about the keys, so nothing holds the mic.
    pub fn helper_lost(&mut self) {
        self.lost = true;
        self.down.clear();
        self.toggled.clear();
        self.release_at = None;
    }
    pub fn helper_back(&mut self) { self.lost = false; }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hold(p: Option<&str>, k: &str) -> Rule { Rule { process: p.map(str::to_string), key: k.into(), mode: Mode::Hold } }
    fn toggle(p: Option<&str>, k: &str) -> Rule { Rule { process: p.map(str::to_string), key: k.into(), mode: Mode::Toggle } }
    fn e(rules: Vec<Rule>, running: &[&str]) -> Engine { let mut e = Engine::new(); e.set_rules(rules); e.processes(running.iter().map(|s| s.to_string())); e }
    fn k(x: &[&str]) -> Vec<String> { x.iter().map(|s| s.to_string()).collect() }

    #[test]
    fn hold_mutes_at_once_and_releases_after_the_delay() {
        let mut m = e(vec![hold(Some("game.exe"), "KeyV")], &["game.exe"]);
        assert!(!m.muted());
        m.keys(&k(&["KeyV"]), 1000);
        assert!(m.muted(), "no delay on the way down");
        m.keys(&k(&[]), 1500);
        assert!(m.muted(), "the end of a word");
        m.tick(1500 + RELEASE_MS - 1);
        assert!(m.muted());
        m.tick(1500 + RELEASE_MS);
        assert!(!m.muted());
    }
    #[test]
    fn pressing_again_during_the_delay_keeps_it_down() {
        let mut m = e(vec![hold(None, "KeyV")], &[]);
        m.keys(&k(&["KeyV"]), 0); m.keys(&k(&[]), 10);
        m.keys(&k(&["KeyV"]), 100);
        m.tick(10 + RELEASE_MS + 1);
        assert!(m.muted(), "the old timer must not fire");
        m.keys(&k(&[]), 400); m.tick(400 + RELEASE_MS);
        assert!(!m.muted());
    }
    #[test]
    fn a_game_key_does_nothing_unless_that_game_runs() {
        let mut m = e(vec![hold(Some("game"), "KeyV")], &["other"]);
        m.keys(&k(&["KeyV"]), 0);
        assert!(!m.muted());
        assert!(m.wanted().is_empty(), "the key is not even looked at");
    }
    #[test]
    fn names_match_however_the_system_spells_them() {
        let mut m = e(vec![hold(Some("Game.EXE"), "KeyV")], &["game"]);
        m.keys(&k(&["KeyV"]), 0);
        assert!(m.muted());
    }
    #[test]
    fn any_app_works_with_no_game_and_alongside_one() {
        let mut m = e(vec![hold(None, "F13"), hold(Some("game"), "KeyV")], &[]);
        assert_eq!(m.wanted(), vec!["F13"]);
        m.keys(&k(&["F13"]), 0);
        assert!(m.muted());
        assert_eq!(m.because(), None);
        m.keys(&k(&[]), 10); m.tick(10 + RELEASE_MS);
        m.processes(["game".to_string()]);
        m.keys(&k(&["KeyV"]), 500);
        assert_eq!(m.because().as_deref(), Some("game"));
    }
    #[test]
    fn toggle_flips_on_each_press_only() {
        let mut m = e(vec![toggle(Some("game"), "KeyV")], &["game"]);
        m.keys(&k(&["KeyV"]), 0);
        assert!(m.muted());
        m.keys(&k(&["KeyV"]), 10);
        m.keys(&k(&[]), 20);
        m.tick(1000);
        assert!(m.muted(), "letting go changes nothing");
        m.keys(&k(&["KeyV"]), 1100);
        assert!(m.muted(), "still the delay");
        m.tick(1100 + RELEASE_MS);
        assert!(!m.muted());
    }
    #[test]
    fn the_game_closing_while_held_lets_go_at_once() {
        let mut m = e(vec![hold(Some("game"), "KeyV")], &["game"]);
        m.keys(&k(&["KeyV"]), 0);
        m.processes(Vec::new());
        assert!(!m.muted());
    }
    #[test]
    fn the_game_closing_while_toggled_lets_go_and_stays_off() {
        let mut m = e(vec![toggle(Some("game"), "KeyV")], &["game"]);
        m.keys(&k(&["KeyV"]), 0); m.keys(&k(&[]), 10);
        m.processes(Vec::new());
        assert!(!m.muted());
        m.processes(["game".to_string()]);
        assert!(!m.muted(), "a game that comes back starts unmuted");
    }
    #[test]
    fn the_game_closing_during_the_delay_ends_it() {
        let mut m = e(vec![hold(Some("game"), "KeyV")], &["game"]);
        m.keys(&k(&["KeyV"]), 0); m.keys(&k(&[]), 10);
        assert!(m.muted());
        m.processes(Vec::new());
        assert!(!m.muted());
    }
    #[test]
    fn helper_loss_lets_go_of_everything() {
        for r in [hold(None, "KeyV"), toggle(None, "KeyV")] {
            let mut m = e(vec![r], &[]);
            m.keys(&k(&["KeyV"]), 0);
            assert!(m.muted());
            m.helper_lost();
            assert!(!m.muted());
            m.keys(&k(&["KeyV"]), 10);
            assert!(!m.muted(), "nothing counts while it is gone");
            m.helper_back();
            assert!(!m.muted(), "and it does not come back muted");
        }
    }
    #[test]
    fn unmuting_by_hand_beats_a_toggle_but_not_a_held_key() {
        let mut m = e(vec![toggle(None, "KeyV"), hold(None, "KeyB")], &[]);
        m.keys(&k(&["KeyV"]), 0); m.keys(&k(&[]), 10);
        m.user_unmute();
        assert!(!m.muted());
        m.keys(&k(&["KeyB"]), 20);
        m.user_unmute();
        assert!(m.muted());
    }
    #[test]
    fn two_keys_held_need_both_up() {
        let mut m = e(vec![hold(None, "KeyV"), hold(Some("game"), "KeyB")], &["game"]);
        m.keys(&k(&["KeyV", "KeyB"]), 0);
        m.keys(&k(&["KeyB"]), 10);
        m.tick(1000);
        assert!(m.muted());
    }
    #[test]
    fn settings_are_checked() {
        let ok = Settings { rules: vec![hold(Some("a"), "KeyV"), hold(None, "F13")], use_portal: false };
        assert!(ok.check().is_ok());
        for bad in [vec![hold(Some("a"), "NoSuchKey")], vec![hold(Some(" "), "KeyV")], vec![hold(Some("a\n"), "KeyV")], vec![hold(Some("a.exe"), "KeyV"), hold(Some("A"), "KeyB")], vec![hold(None, "KeyV"), hold(None, "KeyB")]] {
            assert!(Settings { rules: bad.clone(), use_portal: false }.check().is_err(), "{bad:?}");
        }
    }
}
