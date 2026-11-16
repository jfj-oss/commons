//! The arrival of a new message (app/ui/mega.js `decode`, `sizeTier`, `mentionsMe`, `afterPaint`): the real text stays in place
//! and a copy made of ASCII glyphs sits over it for 340 ms, resolving left to right into the words.
//!
//! This file is the state machine and the maths only (no GPUI), so it can be tested: which messages animate, how far along each
//! one is, what the glyph copy says at a given moment.
use crate::model::Msg;

/// How long the glyphs take to resolve (mega.js `D`).
pub const DURATION: f64 = 0.34;
/// The glyphs the unresolved part is made of (mega.js `SCR`).
pub const SCR: &[u8] = b".:;-/!?'()";
/// Longer messages are not decoded (mega.js: more than 400 characters).
pub const MAX_CHARS: usize = 400;
/// Older than this when it shows up: history paged in, not a new message (seconds).
const FRESH: u64 = 60;
/// More than this many new rows at once is a catch-up, not conversation.
const BURST: usize = 4;
/// Only this many decode at once within `WINDOW` seconds (mega.js `RM.arr`).
const AT_ONCE: usize = 2;
const WINDOW: f64 = 1.5;

/// A message's size tier (mega.js `sizeTier`): text-only messages by length, a mention one tier more. 0 = the room's own size.
pub fn size_tier(m: &Msg, me: &str) -> u8 {
    if m.kind != "text" || m.deleted { return 0; }
    let n = m.text.chars().count();
    if n == 0 { return 0; }
    let t = if n <= 4 { 3 } else if n <= 12 { 2 } else if n <= 18 { 1 } else { 0 };
    (t + u8::from(!m.mine && mentions(&m.text, me))).min(3)
}

/// The size in pixels a tier gives (mega.css `.msg[data-mz]`: 36, 48 and 72 px), or None for the room's own size.
pub fn tier_px(t: u8) -> Option<f32> { match t { 1 => Some(36.), 2 => Some(48.), 3 => Some(72.), _ => None } }

fn word(c: char) -> bool { c.is_alphanumeric() || c == '_' }

/// Does the text mention this person (mega.js `mentionsMe`): `@name` or `@firstname`, whole word, not glued to a word before the @.
pub fn mentions(text: &str, me: &str) -> bool {
    let me = me.trim();
    if me.is_empty() || !text.contains('@') { return false; }
    let mut alts: Vec<String> = vec![me.to_lowercase()];
    if let Some(first) = me.split_whitespace().next() { alts.push(first.to_lowercase()); }
    alts.retain(|a| !a.is_empty());
    let low: Vec<char> = text.to_lowercase().chars().collect();
    for (i, &c) in low.iter().enumerate() {
        if c != '@' { continue; }
        if i > 0 && (word(low[i - 1]) || low[i - 1] == '@') { continue; }
        for a in &alts {
            let a: Vec<char> = a.chars().collect();
            if low.len() >= i + 1 + a.len() && low[i + 1..i + 1 + a.len()] == a[..] && !low.get(i + 1 + a.len()).is_some_and(|&n| word(n)) { return true; }
        }
    }
    false
}

/// One message resolving.
#[derive(Clone, Debug, PartialEq)]
pub struct Arrival { pub id: String, pub t0: f64, pub chars: usize }

/// A room's arrivals. `now` is seconds on any steady clock.
#[derive(Default, Debug)]
pub struct Arrivals {
    pub live: Vec<Arrival>,
    /// When the last few began (the "no more than two at once" rule).
    recent: Vec<f64>,
}

/// What `admit` is told about the moment: the window must be showing the room, and the saver off.
#[derive(Clone, Copy)]
pub struct Moment { pub now: f64, pub wall: u64, pub showing: bool, pub still: bool }

impl Arrivals {
    pub fn clear(&mut self) { self.live.clear(); self.recent.clear(); }
    pub fn is_idle(&self) -> bool { self.live.is_empty() }

    /// The rows just appended to an open room (`fresh`, oldest first) decide which animate. Only an addition to a room that was
    /// already showing animates: history, scrolling and the first load never come here.
    pub fn arrive(&mut self, fresh: &[Msg], mo: Moment) -> usize {
        if mo.still || !mo.showing || fresh.len() > BURST { return 0; }
        let mut n = 0;
        for m in fresh {
            if m.mine || m.deleted || m.kind != "text" || m.text.is_empty() { continue; }
            if mo.wall.saturating_sub(m.ts) > FRESH { continue; }
            self.recent.retain(|t| mo.now - t < WINDOW);
            self.recent.push(mo.now);
            if self.recent.len() > AT_ONCE { continue; }
            let chars = m.text.chars().count();
            if chars == 0 || chars > MAX_CHARS { continue; }
            self.live.push(Arrival { id: m.id.clone(), t0: mo.now, chars });
            n += 1;
        }
        n
    }

    /// How far along a message is, 0..1, or None when it is not animating (done, or never was).
    pub fn progress(&self, id: &str, now: f64) -> Option<f64> {
        let a = self.live.iter().find(|a| a.id == id)?;
        let k = ((now - a.t0) / DURATION).max(0.);
        (k < 1.).then_some(k)
    }

    /// Forget the finished ones. Returns true while something is still moving: the caller keeps drawing frames only then.
    pub fn step(&mut self, now: f64) -> bool {
        self.live.retain(|a| now - a.t0 < DURATION);
        !self.live.is_empty()
    }
}

fn mix(i: usize, tick: u64) -> u32 {
    let mut h = (i as u32).wrapping_mul(0x9E37_79B1) ^ (tick as u32).wrapping_mul(0x85EB_CA6B);
    h ^= h >> 15; h = h.wrapping_mul(0x2C1B_3C6D); h ^= h >> 13; h
}

/// The glyph copy at progress `k`: the first `real` characters are the words, the rest glyphs (whitespace stays), re-rolled each `tick`.
/// Same number of characters as the text, so a line break in it falls where the words' would.
pub fn compose(text: &str, k: f64, tick: u64) -> (String, usize) {
    let total = text.chars().count();
    let real = ((k.clamp(0., 1.) * total as f64).floor() as usize).min(total);
    let mut s = String::with_capacity(text.len());
    for (i, c) in text.chars().enumerate() {
        if i < real || c.is_whitespace() { s.push(c); } else { s.push(SCR[mix(i, tick) as usize % SCR.len()] as char); }
    }
    let bytes = s.char_indices().nth(real).map(|(b, _)| b).unwrap_or(s.len());
    (s, bytes)
}

// ---- thrown reactions (mega.js `throwReaction`, `burst`): an emoji flies to its pill and bursts there; the pill keeps its old number until it lands

/// Seconds an emoji takes to fly (mega.js 560 ms, ease-in) and a burst takes to fade (380 to 680 ms; one length here).
pub const FLIGHT: f64 = 0.56;
pub const BURST_LIFE: f64 = 0.62;
/// At most this many fly at once (mega.js: 3 per paint).
const FLIGHTS: usize = 3;
/// Glyphs in a burst (mega.js: 24, or 10 in lite mode).
pub const BURST_N: usize = 18;

#[derive(Clone, Debug, PartialEq)]
pub struct Flight { pub msg: String, pub emoji: String, pub prev: u64, pub mine: bool, pub t0: f64 }

#[derive(Default, Debug)]
pub struct Flights { pub live: Vec<Flight> }

/// A reaction as the list holds it, for comparing one answer with the last.
pub type Rx = (String, u64, bool);

impl Flights {
    pub fn clear(&mut self) { self.live.clear(); }
    pub fn is_idle(&self) -> bool { self.live.is_empty() }
    /// `before` and `after` are the reactions of one message that was already showing. Starts a flight for each emoji whose count went up.
    pub fn throw(&mut self, msg: &str, before: &[Rx], after: &[Rx], now: f64, allowed: bool) -> usize {
        if !allowed { return 0; }
        let mut n = 0;
        for (e, c, mine) in after {
            let old = before.iter().find(|(x, _, _)| x == e);
            let prev = old.map(|o| o.1).unwrap_or(0);
            if *c > prev && self.live.len() < FLIGHTS && !self.live.iter().any(|f| f.msg == msg && &f.emoji == e) {
                self.live.push(Flight { msg: msg.to_string(), emoji: e.clone(), prev, mine: *mine && !old.is_some_and(|o| o.2), t0: now });
                n += 1;
            }
        }
        n
    }
    /// The number the pill shows: the old one until the emoji lands (None: the pill is hidden, the reaction is new).
    pub fn shown(&self, msg: &str, emoji: &str, now: f64, count: u64) -> Option<u64> {
        match self.live.iter().find(|f| f.msg == msg && f.emoji == emoji) {
            Some(f) if now - f.t0 < FLIGHT => if f.prev > 0 { Some(f.prev) } else { None },
            _ => Some(count),
        }
    }
    pub fn step(&mut self, now: f64) -> bool { self.live.retain(|f| now - f.t0 < FLIGHT + BURST_LIFE); !self.live.is_empty() }
}

/// Where the flying emoji is at `k` (0..1 of the flight), and how large (a multiple of its size): a quadratic arc from `a` through a
/// high control point to `b`, ease-in, swelling to 2.4x in the middle (mega.js keyframes).
pub fn flight_at(a: (f32, f32), b: (f32, f32), k: f32) -> ((f32, f32), f32) {
    let t = k.clamp(0., 1.);
    let e = t * t * (3. - 2. * t) * 0.5 + t * t * 0.5; // ease-in
    let m = ((a.0 + b.0) / 2., a.1.min(b.1) - 90.);
    // the arc passes through m at e = 0.5: control point c such that the curve's midpoint is m
    let c = (2. * m.0 - (a.0 + b.0) / 2., 2. * m.1 - (a.1 + b.1) / 2.);
    let u = 1. - e;
    let p = (u * u * a.0 + 2. * u * e * c.0 + e * e * b.0, u * u * a.1 + 2. * u * e * c.1 + e * e * b.1);
    // size .7 -> 2.4 -> 1
    let s = if e < 0.5 { 0.7 + (2.4 - 0.7) * (e / 0.5) } else { 2.4 + (1. - 2.4) * ((e - 0.5) / 0.5) };
    (p, s)
}

/// Particle `i` of a burst at `k` of its life: where it is relative to the burst's centre and its opacity.
pub fn burst_at(i: usize, k: f32) -> ((f32, f32), f32) {
    let h = mix(i, 77);
    let ang = (h % 6283) as f32 / 1000.;
    let dist = 30. + ((h >> 12) % 100) as f32;
    let ease = 1. - (1. - k.clamp(0., 1.)).powi(3);
    ((ang.cos() * dist * ease, ang.sin() * dist * 0.7 * ease), (1. - k).clamp(0., 1.))
}

/// The burst's glyph `i`.
pub fn burst_glyph(i: usize) -> char { SCR[mix(i, 5) as usize % SCR.len()] as char }

#[cfg(test)]
mod flight_tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn a_higher_count_throws_and_the_pill_waits() {
        let mut f = Flights::default();
        let after = vec![("+1".to_string(), 3u64, false)];
        assert_eq!(f.throw("m1", &[("+1".into(), 2, false)], &after, 5., true), 1);
        assert_eq!(f.shown("m1", "+1", 5.2, 3), Some(2), "still the old number in the air");
        assert_eq!(f.shown("m1", "+1", 5.0 + FLIGHT + 0.01, 3), Some(3), "the true count once it lands");
        assert_eq!(f.shown("m2", "+1", 5.2, 7), Some(7), "other pills are untouched");
        assert!(f.step(5.3));
        assert!(!f.step(5. + FLIGHT + BURST_LIFE + 0.1), "idle again");
        assert!(f.is_idle());
    }
    #[::core::prelude::v1::test]
    fn a_new_reaction_is_hidden_until_it_lands() {
        let mut f = Flights::default();
        f.throw("m1", &[], &[("x".to_string(), 1, true)], 0., true);
        assert_eq!(f.shown("m1", "x", 0.1, 1), None);
        assert_eq!(f.shown("m1", "x", 1., 1), Some(1));
    }
    #[::core::prelude::v1::test]
    fn nothing_flies_when_the_count_is_the_same_or_effects_are_off() {
        let mut f = Flights::default();
        assert_eq!(f.throw("m1", &[("a".into(), 2, false)], &[("a".to_string(), 2, false)], 0., true), 0);
        assert_eq!(f.throw("m1", &[("a".into(), 2, false)], &[("a".to_string(), 1, false)], 0., true), 0, "a reaction taken back");
        assert_eq!(f.throw("m1", &[], &[("a".to_string(), 1, false)], 0., false), 0, "saver or hidden window");
        assert!(f.is_idle());
    }
    #[::core::prelude::v1::test]
    fn at_most_three_fly_at_once() {
        let mut f = Flights::default();
        for i in 0..6 { f.throw(&format!("m{i}"), &[], &[("a".to_string(), 1, false)], 0., true); }
        assert_eq!(f.live.len(), 3);
    }
    #[::core::prelude::v1::test]
    fn the_arc_starts_and_ends_where_it_should_and_swells_between() {
        let (a, b) = ((500., 300.), (100., 400.));
        let (p0, s0) = flight_at(a, b, 0.);
        let (p1, s1) = flight_at(a, b, 1.);
        assert!((p0.0 - a.0).abs() < 1e-3 && (p0.1 - a.1).abs() < 1e-3 && (s0 - 0.7).abs() < 1e-3);
        assert!((p1.0 - b.0).abs() < 1e-3 && (p1.1 - b.1).abs() < 1e-3 && (s1 - 1.).abs() < 1e-3);
        let big = (0..=20).map(|i| flight_at(a, b, i as f32 / 20.).1).fold(0f32, f32::max);
        assert!(big > 2.0 && big <= 2.45, "swells to about 2.4x: {big}");
        // the arc rises above both ends
        let top = (0..=20).map(|i| flight_at(a, b, i as f32 / 20.).0 .1).fold(f32::MAX, f32::min);
        assert!(top < 300. - 60., "arc height {top}");
    }
    #[::core::prelude::v1::test]
    fn a_burst_spreads_and_fades() {
        let (p0, o0) = burst_at(3, 0.);
        let (p1, o1) = burst_at(3, 1.);
        assert!(p0.0.abs() < 1e-3 && p0.1.abs() < 1e-3 && (o0 - 1.).abs() < 1e-3);
        assert!(p1.0.hypot(p1.1) > 15. && o1 == 0.);
        assert!(SCR.contains(&(burst_glyph(4) as u8)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn msg(id: &str, text: &str, ts: u64) -> Msg { Msg { id: id.into(), kind: "text".into(), text: text.into(), ts, name: "Bob".into(), ..Default::default() } }
    fn moment(now: f64) -> Moment { Moment { now, wall: 1000, showing: true, still: false } }

    #[::core::prelude::v1::test]
    fn only_a_new_message_in_a_showing_room_animates() {
        let mut a = Arrivals::default();
        assert_eq!(a.arrive(&[msg("1", "hello there", 1000)], moment(0.)), 1);
        assert!(!a.is_idle());
        let mut b = Arrivals::default();
        assert_eq!(b.arrive(&[msg("1", "hello", 1000)], Moment { showing: false, ..moment(0.) }), 0, "window hidden or another screen");
        assert_eq!(b.arrive(&[msg("1", "hello", 1000)], Moment { still: true, ..moment(0.) }), 0, "battery saver: instant");
        assert_eq!(b.arrive(&[msg("1", "hello", 900)], moment(0.)), 0, "older than a minute is history");
        assert_eq!(b.arrive(&[msg("1", "", 1000)], moment(0.)), 0);
        let mut mine = msg("2", "mine", 1000); mine.mine = true;
        assert_eq!(b.arrive(&[mine], moment(0.)), 0, "what you wrote already rippled as sending");
        let mut gone = msg("3", "x", 1000); gone.deleted = true;
        assert_eq!(b.arrive(&[gone], moment(0.)), 0);
        let mut file = msg("4", "x", 1000); file.kind = "file".into();
        assert_eq!(b.arrive(&[file], moment(0.)), 0);
        assert_eq!(b.arrive(&[msg("5", &"x".repeat(401), 1000)], moment(0.)), 0, "too long");
        assert!(b.is_idle(), "nothing was admitted: no frames");
    }
    #[::core::prelude::v1::test]
    fn a_catch_up_does_not_animate() {
        let mut a = Arrivals::default();
        let five: Vec<Msg> = (0..5).map(|i| msg(&i.to_string(), "hi", 1000)).collect();
        assert_eq!(a.arrive(&five, moment(0.)), 0);
        assert_eq!(a.arrive(&five[..4], moment(0.)), 2, "four is conversation, but only two at once");
    }
    #[::core::prelude::v1::test]
    fn at_most_two_at_once_within_a_moment() {
        let mut a = Arrivals::default();
        assert_eq!(a.arrive(&[msg("1", "a b", 1000)], moment(0.0)), 1);
        assert_eq!(a.arrive(&[msg("2", "a b", 1000)], moment(0.5)), 1);
        assert_eq!(a.arrive(&[msg("3", "a b", 1000)], moment(1.0)), 0);
        assert_eq!(a.arrive(&[msg("4", "a b", 1000)], moment(2.2)), 1, "the old ones have aged out of the window");
    }
    #[::core::prelude::v1::test]
    fn progress_runs_zero_to_one_then_stops() {
        let mut a = Arrivals::default();
        a.arrive(&[msg("1", "hello there", 1000)], moment(10.));
        assert_eq!(a.progress("1", 10.), Some(0.));
        let k = a.progress("1", 10.17).unwrap();
        assert!((k - 0.5).abs() < 0.01);
        assert_eq!(a.progress("1", 10.35), None);
        assert_eq!(a.progress("zzz", 10.1), None);
        assert!(a.step(10.2));
        assert!(!a.step(10.4), "finished: the frame clock can sleep");
        assert!(a.is_idle());
    }
    #[::core::prelude::v1::test]
    fn the_copy_resolves_left_to_right() {
        let t = "hello there";
        let (s0, r0) = compose(t, 0., 0);
        assert_eq!(s0.chars().count(), t.chars().count());
        assert_eq!(r0, 0);
        assert_eq!(s0.chars().nth(5), Some(' '), "whitespace stays");
        assert!(s0.chars().all(|c| c == ' ' || SCR.contains(&(c as u8))));
        let (s5, r5) = compose(t, 0.5, 3);
        assert_eq!(r5, 5);
        assert!(s5.starts_with("hello"));
        assert!(!s5[5..].contains('h') || s5[5..].chars().all(|c| c == ' ' || SCR.contains(&(c as u8))));
        let (s1, r1) = compose(t, 1., 9);
        assert_eq!(s1, t);
        assert_eq!(r1, t.len());
        // the noise is re-rolled each tick
        assert_ne!(compose("abcdefghijklmnop", 0., 1).0, compose("abcdefghijklmnop", 0., 2).0);
        // multi-byte text: the byte split falls on a character boundary
        let (s, r) = compose("héllo wörld", 0.5, 0);
        assert!(s.is_char_boundary(r));
        assert_eq!(s.chars().count(), 11);
    }
    #[::core::prelude::v1::test]
    fn sizes_follow_the_length_and_mentions_land_larger() {
        let m = |t: &str| msg("1", t, 0);
        assert_eq!(size_tier(&m("ok"), "Alice"), 3);
        assert_eq!(size_tier(&m("see you soon"), "Alice"), 2);
        assert_eq!(size_tier(&m("see you tomorrow ok"), "Alice"), 0);
        assert_eq!(size_tier(&m("sixteen chars ok!"), "Alice"), 1);
        assert_eq!(size_tier(&m("hey @Alice please look at this one"), "Alice"), 1);
        assert_eq!(size_tier(&m("hey @alice"), "Alice Liddell"), 3, "ten characters is tier 2, the mention (by first name) makes it 3");
        let mut mine = m("hey @Alice please look at this one"); mine.mine = true;
        assert_eq!(size_tier(&mine, "Alice"), 0, "your own mention does not count");
        let mut f = m("ok"); f.kind = "file".into();
        assert_eq!(size_tier(&f, "Alice"), 0);
        assert_eq!(tier_px(0), None);
        assert_eq!(tier_px(3), Some(72.));
    }
    #[::core::prelude::v1::test]
    fn mention_matching_is_the_webs() {
        assert!(mentions("hi @Alice!", "Alice"));
        assert!(mentions("@alice hi", "Alice"));
        assert!(mentions("hi @Alice", "Alice Liddell"), "the first name counts");
        assert!(mentions("hi @Alice Liddell", "Alice Liddell"));
        assert!(!mentions("hi @Alicia", "Alice"), "whole words only");
        assert!(!mentions("mail me@Alice", "Alice"), "not glued to a word");
        assert!(!mentions("hi Alice", "Alice"), "needs the @");
        assert!(!mentions("hi @@Alice", "Alice"));
        assert!(!mentions("hi @Alice", ""));
    }
}
