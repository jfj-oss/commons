//! Tests only: how much of what a track carries is one known tone. The same idea as `probeTone` in call.js (share of
//! the energy at the tone's frequency, best 100 ms window), measured on PCM from a `NativeAudioStream` instead of an
//! analyser, so the browser's pass marks (above 0.15 heard, below 0.02 not heard) mean the same here.
use std::f64::consts::TAU;

pub struct ToneMeter {
    hz: f64,
    n: u64,
    win: usize,
    in_win: usize,
    s: f64,
    c: f64,
    e: f64,
    pub frames: u64,
    pub best: f64,
    pub rms_max: f64,
}

impl ToneMeter {
    pub fn new(hz: f64) -> ToneMeter { ToneMeter { hz, n: 0, win: 4800, in_win: 0, s: 0., c: 0., e: 0., frames: 0, best: 0., rms_max: 0. } }

    /// Interleaved 16-bit samples; only the first channel is measured.
    pub fn push(&mut self, data: &[i16], sample_rate: u32, channels: u32) {
        self.frames += 1;
        self.win = (sample_rate / 10).max(1) as usize;
        for x in data.iter().step_by(channels.max(1) as usize) {
            let v = *x as f64 / 32768.0;
            let ph = self.n as f64 / sample_rate as f64 * self.hz * TAU;
            self.s += v * ph.sin();
            self.c += v * ph.cos();
            self.e += v * v;
            self.n += 1;
            self.in_win += 1;
            if self.in_win == self.win {
                let n = self.win as f64;
                // a silent window says nothing about the tone; a nearly silent one would make noise look like a tone
                if self.e > 1e-6 * n { self.best = self.best.max((2.0 * (self.s * self.s + self.c * self.c) / n / self.e).min(1.0)); }
                self.rms_max = self.rms_max.max((self.e / n).sqrt());
                self.in_win = 0; self.s = 0.; self.c = 0.; self.e = 0.;
            }
        }
    }

    pub fn result(&self) -> serde_json::Value {
        serde_json::json!({"present": true, "toneShare": self.best, "rms": self.rms_max, "frames": self.frames})
    }
}

/// A sine for a fake microphone: 10 ms of mono 48 kHz, continuing from `n`.
pub fn sine(hz: f64, n: &mut u64, amplitude: f64) -> Vec<i16> {
    let out = (0..480u64).map(|i| ((*n + i) as f64 / 48000.0 * hz * TAU).sin() * amplitude).map(|v| v as i16).collect();
    *n += 480;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_tone_is_found_and_another_is_not() {
        let (mut a, mut b, mut n) = (ToneMeter::new(900.), ToneMeter::new(500.), 0);
        for _ in 0..50 { let f = sine(900., &mut n, 12000.); a.push(&f, 48000, 1); b.push(&f, 48000, 1); }
        assert!(a.best > 0.95, "{}", a.best);
        assert!(b.best < 0.02, "{}", b.best);
    }
    #[test]
    fn silence_is_not_a_tone() {
        let mut m = ToneMeter::new(900.);
        for _ in 0..50 { m.push(&[0i16; 480], 48000, 1); }
        assert_eq!(m.best, 0.);
    }
}
