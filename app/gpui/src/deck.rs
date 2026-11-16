//! The cameras-and-screens column's rules (app/ui/mega-deck.js), without any drawing: the shared tile order, the hold a
//! person has on a tile while dragging it, and the width of the column. The wire format is the web UI's, so web, iOS
//! and this app converge on one order in the same call:
//!   kind `deck.m`  {s, o, n, b, e, a}   s = tile held ("" none), o = order (track ids), n,b = the order's stamp (counter,
//!                                       account), e = 1 on release, a = 1 when it answers a question
//!   kind `deck.q`  {}                   "what is the order?", asked once by someone who just joined
//! Rules a receiver keeps whatever it is sent: only accounts in the call count; unknown tiles are ignored; the local order
//! always holds every current tile exactly once; a higher stamp (counter, then account) wins; a held tile is its holder's
//! until 3 s of silence or until they leave; two simultaneous grabs go to the lower account.
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

pub const HOLD: Duration = Duration::from_millis(3000);
pub const MOVE_GAP: Duration = Duration::from_millis(450);
pub const MAX_TILES: usize = 64;
/// The column is this wide at least (mega-deck.js `limits`) and leaves the chat this much.
pub const MIN_W: f32 = 200.;
pub const CHAT_MIN: f32 = 420.;

/// (min, max, default) of the column for a window this wide.
pub fn limits(window_w: f32) -> (f32, f32, f32) {
    (MIN_W, MIN_W.max(window_w - CHAT_MIN), (270f32).min(window_w * 0.24).round())
}
/// A width the column may have in a window this wide; 0 (never set) is the default.
pub fn clamp_width(w: f32, window_w: f32) -> f32 {
    let (lo, hi, def) = limits(window_w);
    if w <= 0. { def.clamp(lo, hi) } else { w.clamp(lo, hi) }
}
/// What an arrow key on the edge does (a step of 16 px, 64 with shift).
pub fn arrow(w: f32, window_w: f32, dir: f32, big: bool) -> f32 { clamp_width(w + dir * if big { 64. } else { 16. }, window_w) }

fn acct_ok(s: &str) -> bool { s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) }
fn sid_ok(s: &str) -> bool { !s.is_empty() && s.len() <= 200 }
fn cmp(a: &(u64, String), b: &(u64, String)) -> std::cmp::Ordering { a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)) }

#[derive(Default)]
pub struct Deck {
    pub order: Vec<String>,
    pub stamp: (u64, String),
    hi: u64,
    pub holds: HashMap<String, (String, Instant)>,
    /// The tile I am dragging.
    pub drag: Option<String>,
    last_send: Option<Instant>,
    /// A question already asked for this call (its gid).
    asked: String,
}

impl Deck {
    pub fn reset(&mut self) { *self = Deck::default(); }

    /// The order made valid against the tiles that really exist: unknown ids dropped, new ones appended in their own order.
    pub fn layout(&mut self, known: &[String]) -> Vec<String> {
        let set: HashSet<&String> = known.iter().collect();
        let mut seen = HashSet::new();
        let mut next: Vec<String> = self.order.iter().filter(|s| set.contains(s) && seen.insert((*s).clone())).cloned().collect();
        for k in known { if seen.insert(k.clone()) { next.push(k.clone()); } }
        self.order = next.clone();
        next
    }

    fn merged(&self, o: &[String], known: &[String]) -> Vec<String> {
        let set: HashSet<&String> = known.iter().collect();
        let mut seen = HashSet::new();
        let mut out: Vec<String> = o.iter().filter(|s| set.contains(s) && seen.insert((*s).clone())).cloned().collect();
        for s in &self.order { if set.contains(s) && seen.insert(s.clone()) { out.push(s.clone()); } }
        out
    }

    /// The tile `sid` was dropped (or is being dragged) onto the place `to` in the order. Returns the payload to send, when
    /// something changed and someone else is in the call. At most one per MOVE_GAP while dragging; `end` always sends.
    pub fn moved(&mut self, sid: &str, to: usize, me: &str, others: bool, end: bool, now: Instant) -> Option<Value> {
        let cur = self.order.iter().position(|s| s == sid)?;
        let changed = cur != to.min(self.order.len().saturating_sub(1));
        if changed {
            let s = self.order.remove(cur);
            let at = to.min(self.order.len());
            self.order.insert(at, s);
            self.stamp = (self.stamp.0.max(self.hi) + 1, me.to_string());
            self.hi = self.stamp.0;
        }
        if !end && !changed { return None; }
        if !others { return None; }
        if !end && self.last_send.is_some_and(|t| now.duration_since(t) < MOVE_GAP) { return None; }
        self.last_send = Some(now);
        Some(self.payload(sid, end, false))
    }
    pub fn payload(&self, sid: &str, end: bool, answer: bool) -> Value {
        let mut v = json!({"s": sid, "o": self.order, "n": self.stamp.0, "b": self.stamp.1, "e": if end { 1 } else { 0 }});
        if answer { v["a"] = json!(1); }
        v
    }
    pub fn grab(&mut self, sid: &str, me: &str, now: Instant) { self.drag = Some(sid.to_string()); self.holds.insert(sid.to_string(), (me.to_string(), now)); }
    pub fn release(&mut self, sid: &str) { self.drag = None; self.holds.remove(sid); }

    /// Someone else's `deck.m`. `who` are the accounts currently in the call, `known` the tiles that exist. True when the
    /// order or a hold changed (a redraw is due).
    pub fn on_move(&mut self, account: &str, d: &Value, me: &str, who: &HashSet<String>, known: &[String], now: Instant) -> bool {
        if !who.contains(account) { return false; }
        let Some(n) = d["n"].as_u64().filter(|n| *n <= 1 << 40) else { return false };
        let Some(o) = d["o"].as_array().filter(|o| o.len() <= MAX_TILES) else { return false };
        let o: Vec<String> = match o.iter().map(|x| x.as_str().filter(|s| sid_ok(s)).map(String::from)).collect::<Option<Vec<_>>>() { Some(o) => o, None => return false };
        let s = d["s"].as_str().unwrap_or("");
        if !s.is_empty() && !sid_ok(s) { return false; }
        let b = d["b"].as_str().filter(|b| acct_ok(b)).unwrap_or(account).to_string();
        let stamp = (n, b);
        let mut changed = false;
        if !s.is_empty() && known.iter().any(|k| k == s) {
            let h = self.holds.get(s).cloned();
            if d["e"].as_u64() == Some(1) {
                if h.as_ref().is_some_and(|h| h.0 == account) { self.holds.remove(s); changed = true; }
            } else {
                let mine = self.drag.as_deref() == Some(s);
                let free = h.as_ref().map_or(true, |h| h.0 == account || now.duration_since(h.1) > HOLD || !who.contains(&h.0));
                let mut take = true;
                if mine { if account < me { self.drag = None; self.holds.remove(s); } else { take = false; } }
                else if !free && h.as_ref().is_some_and(|h| account > h.0.as_str()) { take = false; }
                if !take { return false; }
                self.holds.insert(s.to_string(), (account.to_string(), now));
                changed = true;
            }
        }
        self.hi = self.hi.max(n);
        if cmp(&stamp, &self.stamp) == std::cmp::Ordering::Greater {
            self.stamp = stamp;
            let mut next = self.merged(&o, known);
            if let Some(dr) = self.drag.clone() {
                let at = self.order.iter().position(|x| *x == dr).unwrap_or(0);
                next.retain(|x| *x != dr);
                next.insert(at.min(next.len()), dr);
            }
            self.order = next;
            changed = true;
        }
        changed
    }
    /// Who holds which tile now (accounts still in the call, other than me, within the hold time).
    pub fn holder(&self, sid: &str, me: &str, who: &HashSet<String>, now: Instant) -> Option<String> {
        self.holds.get(sid).filter(|h| h.0 != me && now.duration_since(h.1) <= HOLD && who.contains(&h.0)).map(|h| h.0.clone())
    }
    /// A newcomer asks once per call, when there are other people.
    pub fn should_ask(&mut self, gid: &str, others: bool) -> bool {
        if !others || self.asked == gid { return false; }
        self.asked = gid.to_string();
        true
    }
    /// Someone asked: answer only if somebody ever reordered anything.
    pub fn should_answer(&self) -> bool { self.stamp.0 != 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn v(a: &[&str]) -> Vec<String> { a.iter().map(|s| s.to_string()).collect() }
    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn width_limits_follow_the_web_ui() {
        assert_eq!(limits(1600.), (200., 1180., 270.));
        assert_eq!(limits(700.), (200., 280., 168.));
        assert_eq!(clamp_width(100., 1600.), 200.);
        assert_eq!(clamp_width(5000., 1600.), 1180.);
        assert_eq!(clamp_width(0., 1600.), 270.);
        assert_eq!(arrow(270., 1600., 1., false), 286.);
        assert_eq!(arrow(200., 1600., -1., true), 200.);
        // a window too narrow for 420 px of chat still keeps the minimum
        assert_eq!(clamp_width(900., 500.), 200.);
    }
    #[test]
    fn layout_keeps_every_tile_once() {
        let mut d = Deck::default();
        assert_eq!(d.layout(&v(&["x", "y", "z"])), v(&["x", "y", "z"]));
        d.order = v(&["z", "gone", "x", "x"]);
        assert_eq!(d.layout(&v(&["x", "y", "z"])), v(&["z", "x", "y"]));
    }
    #[test]
    fn my_move_bumps_the_stamp_and_sends_once_per_gap() {
        let mut d = Deck::default();
        d.layout(&v(&["x", "y", "z"]));
        let t = Instant::now();
        let p = d.moved("z", 0, A, true, false, t).unwrap();
        assert_eq!(p["o"], json!(["z", "x", "y"]));
        assert_eq!(p["n"], 1);
        assert!(d.moved("y", 0, A, true, false, t + Duration::from_millis(100)).is_none(), "within the gap");
        assert_eq!(d.order, v(&["y", "z", "x"]));
        let p = d.moved("y", 0, A, true, true, t + Duration::from_millis(120)).unwrap();
        assert_eq!(p["e"], 1);
        assert!(d.moved("x", 2, A, false, true, t).is_none(), "nobody else: nothing to tell");
    }
    #[test]
    fn higher_stamp_wins_and_unknown_tiles_are_ignored() {
        let mut d = Deck::default();
        d.layout(&v(&["x", "y", "z"]));
        let who: HashSet<String> = [B.to_string()].into();
        let t = Instant::now();
        let msg = json!({"s": "", "o": ["z", "nope", "y", "x"], "n": 3, "b": B});
        assert!(d.on_move(B, &msg, A, &who, &v(&["x", "y", "z"]), t));
        assert_eq!(d.order, v(&["z", "y", "x"]));
        // an older stamp changes nothing
        let old = json!({"s": "", "o": ["x", "y", "z"], "n": 2, "b": B});
        assert!(!d.on_move(B, &old, A, &who, &v(&["x", "y", "z"]), t));
        assert_eq!(d.order, v(&["z", "y", "x"]));
        // someone not in the call is ignored
        assert!(!d.on_move(A, &json!({"s": "", "o": ["x"], "n": 9}), B, &who, &v(&["x", "y", "z"]), t));
        // junk is ignored
        assert!(!d.on_move(B, &json!({"o": "x", "n": 9}), A, &who, &v(&["x"]), t));
        assert!(!d.on_move(B, &json!({"o": [1], "n": 9}), A, &who, &v(&["x"]), t));
    }
    #[test]
    fn holds_go_to_the_lower_account_and_expire() {
        let mut d = Deck::default();
        d.layout(&v(&["x", "y"]));
        let who: HashSet<String> = [B.to_string(), A.to_string()].into();
        let t = Instant::now();
        // B holds x; A (lower) is dragging x too: B (higher) loses it to me when I am A
        d.grab("x", A, t);
        let m = json!({"s": "x", "o": ["x", "y"], "n": 1, "b": B});
        assert!(!d.on_move(B, &m, A, &who, &v(&["x", "y"]), t), "I am lower: my grab stands");
        assert_eq!(d.drag.as_deref(), Some("x"));
        // as B (higher) I give way to A
        let mut e = Deck::default();
        e.layout(&v(&["x", "y"]));
        e.grab("x", B, t);
        assert!(e.on_move(A, &json!({"s": "x", "o": ["x", "y"], "n": 1, "b": A}), B, &who, &v(&["x", "y"]), t));
        assert!(e.drag.is_none());
        assert_eq!(e.holder("x", B, &who, t).as_deref(), Some(A));
        assert!(e.holder("x", B, &who, t + Duration::from_secs(4)).is_none(), "3 s of silence frees it");
    }
    #[test]
    fn a_newcomer_asks_once_per_call() {
        let mut d = Deck::default();
        assert!(!d.should_ask("g", false));
        assert!(d.should_ask("g", true));
        assert!(!d.should_ask("g", true));
        assert!(!d.should_answer());
    }
}
