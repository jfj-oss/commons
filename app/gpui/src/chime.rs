//! The soft chime for a new message (app/ui/app.js `chime`): two sine notes, 660 Hz and 990 Hz (the second 85 ms later),
//! one shared envelope (a 15 ms attack to 0.09, then an exponential fall to nothing at 0.55 s), 0.6 s in all. Made here
//! as a WAV, written once to the data folder and played by whichever player the computer has (`pw-play`, `paplay`,
//! `aplay`; macOS: `afplay`).
//!
//! Tests never make a sound: `MEGA_GPUI_AUDIO_SINK=null` (the pattern playback.rs uses) discards it, and
//! `MEGA_GPUI_CHIME_FILE=<file>` appends one JSON line per chime instead so a test can see that it was due and why.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const RATE: u32 = 44_100;
/// At most one chime this often (the web's `Date.now() - chimeAt < 2000`).
pub const GAP: Duration = Duration::from_secs(2);

/// The chime as 16-bit mono samples.
pub fn samples() -> Vec<i16> {
    let total = (0.6 * RATE as f64) as usize;
    let tau = std::f64::consts::TAU;
    (0..total).map(|i| {
        let t = i as f64 / RATE as f64;
        // the gain node: exponentialRampToValueAtTime(0.09, 0.015) from 0.0001, then to 0.0001 at 0.55
        let g = if t < 0.015 { 0.0001 * (0.09f64 / 0.0001).powf(t / 0.015) } else if t < 0.55 { 0.09 * (0.0001f64 / 0.09).powf((t - 0.015) / 0.535) } else { 0.0001 };
        let mut x = (t * 660. * tau).sin();
        if t >= 0.085 { x += ((t - 0.085) * 990. * tau).sin(); }
        (x * g * 32767.).clamp(-32767., 32767.) as i16
    }).collect()
}

pub fn wav_bytes() -> Vec<u8> {
    let pcm = samples();
    let data = (pcm.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + pcm.len() * 2);
    b.extend(b"RIFF"); b.extend((36 + data).to_le_bytes()); b.extend(b"WAVEfmt "); b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes()); b.extend(1u16.to_le_bytes()); b.extend(RATE.to_le_bytes()); b.extend((RATE * 2).to_le_bytes());
    b.extend(2u16.to_le_bytes()); b.extend(16u16.to_le_bytes()); b.extend(b"data"); b.extend(data.to_le_bytes());
    for x in pcm { b.extend(x.to_le_bytes()); }
    b
}

/// What the chime does on this run, decided from the environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out { Null, File(PathBuf), Speakers(PathBuf) }
impl Out {
    pub fn choose(dir: &Path) -> Out {
        if let Some(f) = std::env::var_os("MEGA_GPUI_CHIME_FILE") { return Out::File(f.into()); }
        if crate::playback::null_sink() || std::env::var("MEGA_GPUI_CALL_AUDIO").as_deref() == Ok("none") { return Out::Null; }
        Out::Speakers(dir.join("chime.wav"))
    }
}

/// The chime with its own throttle.
pub struct Chime { out: Out, last: Option<Instant> }
impl Chime {
    pub fn new(dir: &Path) -> Chime { Chime { out: Out::choose(dir), last: None } }
    /// Plays unless one played less than [`GAP`] ago. `why` goes into a test's record. Returns whether it counted as played.
    pub fn play(&mut self, why: &str, room: &str) -> bool {
        if self.last.is_some_and(|t| t.elapsed() < GAP) { return false; }
        self.last = Some(Instant::now());
        match &self.out {
            Out::Null => {}
            Out::File(f) => {
                use std::io::Write;
                if let Ok(mut h) = std::fs::OpenOptions::new().create(true).append(true).open(f) { let _ = writeln!(h, "{}", serde_json::json!({"tone": "chime", "why": why, "room": room})); }
            }
            Out::Speakers(wav) => {
                let want = wav_bytes();
                if std::fs::read(wav).ok().as_deref() != Some(want.as_slice()) { let _ = std::fs::write(wav, &want); }
                let wav = wav.clone();
                // fire and forget: the first player that exists plays it; none is not an error
                std::thread::spawn(move || {
                    for (prog, args) in [("pw-play", vec![]), ("paplay", vec![]), ("aplay", vec!["-q"]), ("afplay", vec![])] {
                        if std::process::Command::new(prog).args(&args).arg(&wav).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success()) { return; }
                    }
                });
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_notes_over_six_tenths_of_a_second_and_never_loud() {
        let s = samples();
        assert_eq!(s.len(), (0.6 * RATE as f64) as usize);
        let peak = s.iter().map(|x| x.unsigned_abs()).max().unwrap() as f64 / 32767.;
        assert!(peak > 0.05 && peak < 0.19, "soft: two notes of at most 0.09 each, peak {peak}");
        assert!(s[0].abs() < 40, "starts from nothing");
        let tail = s[(0.57 * RATE as f64) as usize..].iter().map(|x| x.unsigned_abs()).max().unwrap();
        assert!(tail < 20, "fades out, tail {tail}");
        // the second note arrives at 85 ms: before it only 660 Hz is there
        let zero_cross = |a: usize, b: usize| s[a..b].windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count();
        let (a, b) = ((0.02 * RATE as f64) as usize, (0.08 * RATE as f64) as usize);
        let hz = zero_cross(a, b) as f64 / 2. / ((b - a) as f64 / RATE as f64);
        assert!((hz - 660.).abs() < 40., "first note {hz} Hz");
    }

    #[test]
    fn the_wav_is_a_proper_file() {
        let w = wav_bytes();
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(w.len(), 44 + samples().len() * 2);
        assert_eq!(u32::from_le_bytes(w[4..8].try_into().unwrap()) as usize, w.len() - 8);
    }

    #[test]
    fn a_test_run_records_the_chime_and_sounds_nothing_and_it_comes_once_per_two_seconds() {
        let f = std::env::temp_dir().join(format!("mega-chime-test-{}", std::process::id()));
        let _ = std::fs::remove_file(&f);
        let mut c = Chime { out: Out::File(f.clone()), last: None };
        assert!(c.play("message", "g1"));
        assert!(!c.play("mention", "g1"), "throttled");
        let text = std::fs::read_to_string(&f).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(text.contains("\"why\":\"message\""));
        let mut n = Chime { out: Out::Null, last: None };
        assert!(n.play("message", "g"));
        let _ = std::fs::remove_file(&f);
    }
}
