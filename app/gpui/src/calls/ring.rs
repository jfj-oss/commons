//! Rings and direct calls (CALLS-PLAN.md step 7): the rules of app/ui/mega-ring.js and mega-direct.js, without any drawing.
//!
//! The core already enforces who may ring (a current member, fresh, not too many, never from yourself or your other
//! devices) and sends and reads the rings (`ring_send`, `ring_end`, `ring_poll`, `direct_call`); the wire is the same as the
//! web's, so a ring from Chromium or iOS shows here and the other way round. This file only keeps what the window shows:
//!
//! - incoming **cards** (at most three are drawn): one per ring, gone when answered, declined, taken back by the caller,
//!   after 30 seconds, when we are already in that call, or when the room is gone;
//! - the **missed-call** notes: a card that went by itself (nobody answered it, or the caller gave up) leaves one line;
//! - our own **rings** that are still ringing (`*` for everyone, or one account), ended when we leave the call, when the
//!   person arrives, or after 30 seconds;
//! - **direct calls** we started: the other person has 30 seconds (plus a little for the invite) to come, then it ends;
//! - when the ring sound is due: every 2.6 s while a card shows, never when sound is off, never in tests.
//!
//! Time is passed in (seconds, any origin) so every rule is a plain unit test.
use std::collections::{BTreeMap, HashSet};

pub const TTL: f64 = 30.0;
pub const TONE_EVERY: f64 = 2.6;
/// A direct call waits a little longer than a ring: the invite has to reach the other app first (mega-direct.js `TTL + 3`).
pub const DIRECT_WAIT: f64 = TTL + 3.0;
/// How many cards are drawn at once.
pub const SHOWN: usize = 3;
/// Missed-call notes kept (the newest).
pub const MISSED_KEPT: usize = 5;

/// One entry of `ring_poll`'s `events`.
#[derive(Clone, Debug, PartialEq)]
pub struct Ev { pub id: u64, pub gid: String, pub account: String, pub ring: String, pub ts: f64, pub end: bool, pub direct: bool }
impl Ev {
    pub fn parse(v: &serde_json::Value) -> Option<Ev> {
        Some(Ev {
            id: v["id"].as_u64()?, gid: v["gid"].as_str()?.to_string(), account: v["account"].as_str()?.to_string(), ring: v["ring"].as_str()?.to_string(),
            ts: v["ts"].as_f64().unwrap_or(0.), end: v["end"].as_bool().unwrap_or(false), direct: v["direct"].as_bool().unwrap_or(false),
        })
    }
}

/// What the snapshot says about the room that is ringing.
#[derive(Clone, Debug, PartialEq)]
pub struct Room { pub cid: String, pub one_to_one: bool, pub room: String, pub comm: String }

#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub ring: String, pub gid: String, pub cid: String, pub one_to_one: bool, pub room: String, pub comm: String,
    pub account: String, pub name: String, pub direct: bool, pub until: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Missed { pub who: String, pub room: String, pub one_to_one: bool, pub at: f64, pub gave_up: bool }
impl Missed {
    pub fn line(&self) -> String {
        if self.one_to_one { format!("Missed call from {}", self.who) } else { format!("Missed call from {} in {}", self.who, self.room) }
    }
}

/// A ring we sent that is still ringing.
#[derive(Clone, Debug, PartialEq)]
pub struct Out { pub gid: String, pub id: String, pub to: String, pub until: f64 }

/// A direct call we started and are waiting on.
#[derive(Clone, Debug, PartialEq)]
pub struct Dial { pub room: String, pub peer: String, pub name: String, pub started: f64, pub until: f64 }

/// What `sweep` found out and the caller has to act on.
#[derive(Default, Debug, PartialEq)]
pub struct Swept {
    /// Rings of ours to end at the core (`ring_end`).
    pub end_outs: Vec<Out>,
    /// Direct calls nobody answered: leave the call and say so.
    pub no_answer: Vec<Dial>,
    pub changed: bool,
}

#[derive(Default)]
pub struct Book {
    pub cards: Vec<Card>,
    declined: HashSet<String>,
    /// The id of the last `ring_poll` event seen.
    pub since: u64,
    pub missed: Vec<Missed>,
    pub outs: BTreeMap<String, Out>,
    pub dials: BTreeMap<String, Dial>,
    last_tone: Option<f64>,
}

impl Book {
    /// A ring event. Returns true when a new card was made (the caller then looks up the caller's name and notifies).
    /// `room` is None when the snapshot has no such room (a ring for a room we are not in is not ours to show); `my_gid` is
    /// the room of the call we are in.
    pub fn take(&mut self, ev: &Ev, room: Option<Room>, my_gid: Option<&str>, now: f64) -> bool {
        self.since = self.since.max(ev.id);
        if ev.end {
            // the caller took it back: a card we had not answered becomes a missed call
            if let Some(i) = self.cards.iter().position(|c| c.ring == ev.ring && c.account == ev.account) {
                let c = self.cards.remove(i);
                self.note_missed(&c, now, true);
            }
            return false;
        }
        if self.cards.iter().any(|c| c.ring == ev.ring) || self.declined.contains(&ev.ring) { return false; }
        let Some(room) = room else { return false };
        if my_gid == Some(ev.gid.as_str()) { return false; }          // already in this call: nothing to answer
        let age = (now - ev.ts).max(0.);
        if age >= TTL { return false; }
        self.cards.push(Card {
            ring: ev.ring.clone(), gid: ev.gid.clone(), cid: room.cid, one_to_one: room.one_to_one, room: room.room, comm: room.comm,
            account: ev.account.clone(), name: ev.account.chars().take(6).collect(), direct: ev.direct, until: now + TTL - age,
        });
        true
    }
    pub fn set_name(&mut self, ring: &str, name: &str) { if let Some(c) = self.cards.iter_mut().find(|c| c.ring == ring) { c.name = name.to_string(); } }

    fn note_missed(&mut self, c: &Card, at: f64, gave_up: bool) {
        self.missed.push(Missed { who: c.name.clone(), room: c.room.clone(), one_to_one: c.one_to_one, at, gave_up });
        let n = self.missed.len();
        if n > MISSED_KEPT { self.missed.drain(..n - MISSED_KEPT); }
    }

    /// Joining from a card: the card goes (the caller joins the room).
    pub fn accept(&mut self, ring: &str) -> Option<Card> { self.cards.iter().position(|c| c.ring == ring).map(|i| self.cards.remove(i)) }
    /// Declining remembers the ring, so a late copy of the event does not bring the card back.
    pub fn decline(&mut self, ring: &str) -> Option<Card> {
        let c = self.accept(ring)?;
        self.declined.insert(c.ring.clone());
        Some(c)
    }
    /// All the cards of a room go (we joined it some other way).
    pub fn answered(&mut self, gid: &str) { self.cards.retain(|c| c.gid != gid); }

    pub fn dismiss_missed(&mut self) { self.missed.clear(); }

    /// The cards that are drawn.
    pub fn shown(&self) -> &[Card] { &self.cards[..self.cards.len().min(SHOWN)] }

    /// Expiry and the rules that tie everything to the call we are in. `my_gid` is the room of our call (None when not in
    /// one), `exists` says whether the snapshot still has a room, `in_call` whether an account is in our call right now.
    pub fn sweep(&mut self, now: f64, my_gid: Option<&str>, exists: &dyn Fn(&str) -> bool, in_call: &dyn Fn(&str) -> bool) -> Swept {
        let mut out = Swept::default();
        let mut gone = vec![];
        self.cards.retain(|c| {
            if now >= c.until { gone.push(c.clone()); return false; }                    // nobody answered: missed
            if my_gid == Some(c.gid.as_str()) || !exists(&c.gid) { out.changed = true; return false; }
            true
        });
        for c in gone { self.note_missed(&c, now, false); out.changed = true; }
        let keys: Vec<String> = self.outs.keys().cloned().collect();
        for k in keys {
            let o = self.outs[&k].clone();
            if now >= o.until { self.outs.remove(&k); out.changed = true; continue; }
            // we left the call (or went to another): the ring ends. They came: stop ringing them.
            let left = my_gid != Some(o.gid.as_str());
            let came = k != "*" && my_gid == Some(o.gid.as_str()) && in_call(&k);
            if left || came { self.outs.remove(&k); out.changed = true; out.end_outs.push(o); }
        }
        let rooms: Vec<String> = self.dials.keys().cloned().collect();
        for room in rooms {
            let d = self.dials[&room].clone();
            if in_call(&d.peer) && my_gid == Some(room.as_str()) { self.dials.remove(&room); out.changed = true; continue; }          // they came
            if my_gid != Some(room.as_str()) { if now - d.started > 3. { self.dials.remove(&room); out.changed = true; } continue; }   // we left
            if now >= d.until { self.dials.remove(&room); out.no_answer.push(d); out.changed = true; }
        }
        // a note nobody read fades after a while
        let before = self.missed.len();
        self.missed.retain(|m| now - m.at < 600.);
        if self.missed.len() != before { out.changed = true; }
        out
    }

    /// Is the ring sound due? Every 2.6 s while any card shows; the sound switch is the caller's.
    pub fn tone_due(&mut self, now: f64) -> bool {
        if self.cards.is_empty() { self.last_tone = None; return false; }
        if self.last_tone.map_or(true, |t| now - t >= TONE_EVERY) { self.last_tone = Some(now); return true; }
        false
    }
}

// ------------------------------------------------------------------------------------------------ the sound

/// What a ring's sound does, decided from the environment and the person's choice. Tests set `MEGA_GPUI_RING_SOUND_FILE`
/// (each time the sound would play, a line goes to that file and nothing is heard) or run without an audio device
/// (`MEGA_GPUI_CALL_AUDIO=none`): in neither case does anything reach a speaker.
pub enum Chime { Off, File(std::path::PathBuf), Speakers(std::path::PathBuf) }
impl Chime {
    pub fn choose(sound_on: bool, dir: &std::path::Path) -> Chime {
        if !sound_on { return Chime::Off; }
        if let Some(f) = std::env::var_os("MEGA_GPUI_RING_SOUND_FILE") { return Chime::File(f.into()); }
        if std::env::var("MEGA_GPUI_CALL_AUDIO").as_deref() == Ok("none") { return Chime::Off; }
        Chime::Speakers(dir.join("ring.wav"))
    }
    pub fn play(&self, note: &str) {
        match self {
            Chime::Off => {}
            Chime::File(f) => {
                use std::io::Write;
                if let Ok(mut h) = std::fs::OpenOptions::new().create(true).append(true).open(f) { let _ = writeln!(h, "{{\"tone\":\"ring\",\"for\":{}}}", serde_json::json!(note)); }
            }
            Chime::Speakers(wav) => {
                if !wav.exists() { let _ = std::fs::write(wav, wav_bytes()); }
                let wav = wav.clone();
                // fire and forget: the first player that exists plays it; none is not an error
                std::thread::spawn(move || {
                    for (prog, args) in [("pw-play", vec![]), ("paplay", vec![]), ("aplay", vec!["-q"])] {
                        if std::process::Command::new(prog).args(&args).arg(&wav).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success()) { return; }
                    }
                });
            }
        }
    }
}

/// The ring: the web's four notes (740, 988, 740, 988 Hz, 0.3 s each, a soft attack and release), as a 16-bit mono WAV.
pub fn wav_bytes() -> Vec<u8> {
    let rate = 44100u32;
    let notes = [(740.0f64, 0.0f64), (988.0, 0.16), (740.0, 0.5), (988.0, 0.66)];
    let total = (0.96 * rate as f64) as usize;
    let mut pcm = vec![0f64; total];
    for (hz, start) in notes {
        let s0 = (start * rate as f64) as usize;
        let len = (0.3 * rate as f64) as usize;
        for i in 0..len {
            let t = i as f64 / rate as f64;
            let env = (1.0 - i as f64 / len as f64).powi(2) * (i as f64 / (0.02 * rate as f64)).min(1.0);
            if let Some(x) = pcm.get_mut(s0 + i) { *x += (t * hz * std::f64::consts::TAU).sin() * env * 0.18; }
        }
    }
    let mut b = Vec::with_capacity(44 + total * 2);
    let data = (total * 2) as u32;
    b.extend(b"RIFF"); b.extend((36 + data).to_le_bytes()); b.extend(b"WAVEfmt "); b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes()); b.extend(1u16.to_le_bytes()); b.extend(rate.to_le_bytes()); b.extend((rate * 2).to_le_bytes());
    b.extend(2u16.to_le_bytes()); b.extend(16u16.to_le_bytes()); b.extend(b"data"); b.extend(data.to_le_bytes());
    for x in pcm { b.extend((((x.clamp(-1., 1.)) * 32767.0) as i16).to_le_bytes()); }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(id: u64, ring: &str, gid: &str, who: &str, ts: f64) -> Ev { Ev { id, gid: gid.into(), account: who.into(), ring: ring.into(), ts, end: false, direct: false } }
    fn room() -> Option<Room> { Some(Room { cid: "c1".into(), one_to_one: false, room: "porch".into(), comm: "club".into() }) }
    fn all(_: &str) -> bool { true }
    fn nobody(_: &str) -> bool { false }

    #[test]
    fn a_ring_makes_a_card_that_lasts_thirty_seconds_from_when_it_was_sent() {
        let mut b = Book::default();
        assert!(b.take(&ev(1, "r1", "g", "alice-account", 100.), room(), None, 105.));
        assert_eq!(b.cards[0].until, 130., "the 30 s run from the ring, not from when we saw it");
        assert_eq!(b.cards[0].name, "alice-");
        assert!(!b.take(&ev(2, "r1", "g", "alice-account", 100.), room(), None, 106.), "the same ring twice is one card");
        assert!(!b.take(&ev(3, "r2", "g", "a", 60.), room(), None, 106.), "an old ring never shows");
        assert_eq!(b.since, 3);
        b.set_name("r1", "Alice");
        assert_eq!(b.cards[0].name, "Alice");
    }

    #[test]
    fn nothing_to_answer_when_already_in_that_call_or_the_room_is_unknown() {
        let mut b = Book::default();
        assert!(!b.take(&ev(1, "r1", "g", "a", 10.), room(), Some("g"), 10.));
        assert!(!b.take(&ev(2, "r2", "g", "a", 10.), None, None, 10.));
        assert!(b.cards.is_empty());
    }

    #[test]
    fn declining_is_remembered_and_leaves_no_missed_call() {
        let mut b = Book::default();
        b.take(&ev(1, "r1", "g", "a", 10.), room(), None, 10.);
        assert!(b.decline("r1").is_some());
        assert!(!b.take(&ev(1, "r1", "g", "a", 10.), room(), None, 11.), "a late copy does not bring it back");
        assert!(b.missed.is_empty());
    }

    #[test]
    fn the_card_goes_after_thirty_seconds_and_a_missed_call_is_noted() {
        let mut b = Book::default();
        b.take(&ev(1, "r1", "g", "alice", 100.), room(), None, 100.);
        b.set_name("r1", "Alice");
        let s = b.sweep(129., None, &all, &nobody);
        assert!(b.cards.len() == 1 && !s.changed);
        let s = b.sweep(130., None, &all, &nobody);
        assert!(b.cards.is_empty() && s.changed);
        assert_eq!(b.missed.len(), 1);
        assert_eq!(b.missed[0].line(), "Missed call from Alice in porch");
        assert!(!b.missed[0].gave_up);
    }

    #[test]
    fn the_caller_giving_up_ends_the_card_and_counts_as_missed() {
        let mut b = Book::default();
        b.take(&ev(1, "r1", "g", "alice", 100.), room(), None, 100.);
        b.take(&Ev { id: 2, end: true, ..ev(2, "r1", "g", "alice", 110.) }, room(), None, 110.);
        assert!(b.cards.is_empty());
        assert_eq!(b.missed.len(), 1);
        assert!(b.missed[0].gave_up);
        // an end for a ring we never saw, or from someone else, changes nothing
        b.take(&Ev { id: 3, end: true, ..ev(3, "zz", "g", "alice", 110.) }, room(), None, 110.);
        assert_eq!(b.missed.len(), 1);
    }

    #[test]
    fn a_direct_call_reads_as_a_call_from_a_person() {
        let mut b = Book::default();
        let one = Some(Room { cid: "d".into(), one_to_one: true, room: "call".into(), comm: "".into() });
        b.take(&Ev { direct: true, ..ev(1, "r1", "d", "bob", 0.) }, one, None, 0.);
        b.set_name("r1", "Bob");
        b.sweep(31., None, &all, &nobody);
        assert_eq!(b.missed[0].line(), "Missed call from Bob");
    }

    #[test]
    fn joining_the_room_another_way_takes_the_card_away_without_a_missed_call() {
        let mut b = Book::default();
        b.take(&ev(1, "r1", "g", "a", 10.), room(), None, 10.);
        b.take(&ev(2, "r2", "h", "b", 10.), room(), None, 10.);
        b.sweep(12., Some("g"), &all, &nobody);
        assert_eq!(b.cards.len(), 1);
        assert_eq!(b.cards[0].gid, "h");
        assert!(b.missed.is_empty());
        b.sweep(13., None, &|g| g != "h", &nobody);
        assert!(b.cards.is_empty() && b.missed.is_empty(), "a room that is gone is not a missed call");
    }

    #[test]
    fn only_three_cards_are_drawn() {
        let mut b = Book::default();
        for i in 0..5 { b.take(&ev(i + 1, &format!("r{i}"), "g", "a", 10.), room(), None, 10.); }
        assert_eq!((b.cards.len(), b.shown().len()), (5, SHOWN));
    }

    #[test]
    fn our_rings_end_when_we_leave_when_they_come_and_after_thirty_seconds() {
        let mut b = Book::default();
        b.outs.insert("*".into(), Out { gid: "g".into(), id: "x1".into(), to: "".into(), until: 130. });
        b.outs.insert("bob".into(), Out { gid: "g".into(), id: "x2".into(), to: "bob".into(), until: 130. });
        b.outs.insert("cy".into(), Out { gid: "g".into(), id: "x3".into(), to: "cy".into(), until: 130. });
        // Bob arrives: only his ring ends
        let s = b.sweep(110., Some("g"), &all, &|a| a == "bob");
        assert_eq!(s.end_outs.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(), ["x2"]);
        // we leave the call: the rest end at the core
        let s = b.sweep(112., None, &all, &nobody);
        assert_eq!(s.end_outs.len(), 2);
        assert!(b.outs.is_empty());
        // and a ring left alone just runs out (no ring_end needed: the core's own limit ends it)
        b.outs.insert("*".into(), Out { gid: "g".into(), id: "x4".into(), to: "".into(), until: 130. });
        let s = b.sweep(130., Some("g"), &all, &nobody);
        assert!(s.end_outs.is_empty() && b.outs.is_empty());
    }

    #[test]
    fn a_direct_call_nobody_answers_ends_after_thirty_three_seconds() {
        let mut b = Book::default();
        b.dials.insert("room".into(), Dial { room: "room".into(), peer: "bob".into(), name: "Bob".into(), started: 100., until: 100. + DIRECT_WAIT });
        assert!(b.sweep(120., Some("room"), &all, &nobody).no_answer.is_empty());
        let s = b.sweep(133., Some("room"), &all, &nobody);
        assert_eq!(s.no_answer.len(), 1);
        assert_eq!(s.no_answer[0].name, "Bob");
        assert!(b.dials.is_empty());
        // answered in time: nothing happens
        b.dials.insert("room".into(), Dial { room: "room".into(), peer: "bob".into(), name: "Bob".into(), started: 200., until: 233. });
        assert!(b.sweep(210., Some("room"), &all, &|a| a == "bob").no_answer.is_empty());
        assert!(b.dials.is_empty());
        // we hung up first: forgotten without a notice
        b.dials.insert("room".into(), Dial { room: "room".into(), peer: "bob".into(), name: "Bob".into(), started: 300., until: 333. });
        assert!(b.sweep(310., None, &all, &nobody).no_answer.is_empty());
        assert!(b.dials.is_empty());
    }

    #[test]
    fn the_sound_repeats_every_2_6_seconds_only_while_a_card_shows() {
        let mut b = Book::default();
        assert!(!b.tone_due(0.));
        b.take(&ev(1, "r1", "g", "a", 10.), room(), None, 10.);
        assert!(b.tone_due(10.));
        assert!(!b.tone_due(12.5));
        assert!(b.tone_due(12.7));
        b.accept("r1");
        assert!(!b.tone_due(20.));
        b.take(&ev(2, "r2", "g", "a", 21.), room(), None, 21.);
        assert!(b.tone_due(21.), "a new ring chimes at once");
    }

    #[test]
    fn the_chime_is_silent_when_off_in_tests_and_records_its_attempts_in_the_hook_file() {
        let dir = std::env::temp_dir().join(format!("mega-ring-chime-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(matches!(Chime::choose(false, &dir), Chime::Off));
        let f = dir.join("sound.jsonl");
        std::env::set_var("MEGA_GPUI_RING_SOUND_FILE", &f);
        let c = Chime::choose(true, &dir);
        assert!(matches!(c, Chime::File(_)));
        c.play("Alice");
        c.play("Alice");
        assert_eq!(std::fs::read_to_string(&f).unwrap().lines().count(), 2);
        assert!(matches!(Chime::choose(false, &dir), Chime::Off), "sound off: nothing is even recorded");
        std::env::remove_var("MEGA_GPUI_RING_SOUND_FILE");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_ring_wav_is_a_valid_short_file() {
        let w = wav_bytes();
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(&w[8..12], b"WAVE");
        assert!(w.len() > 44 + 40000 && w.len() < 44 + 100000);
    }
}
