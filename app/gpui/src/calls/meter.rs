//! The microphone level, for the call settings' "Test microphone" meter and for noticing a microphone that sends only
//! silence. Visual only: nothing is ever played. In a call the level is the sender's own `media-source` `audioLevel`
//! (what libwebrtc really sends, on every platform). Outside a call, on Linux, a `parec` child records the chosen
//! source for as long as the meter is wanted (the settings panel is open), so a person can check a microphone before
//! joining; elsewhere the meter works during a call only.

/// Below this the microphone is sending digital silence (a room is never below it; a muted source is exactly 0).
pub const SILENT: f32 = 0.0003;
/// How long that has to last before the app says so.
pub const QUIET_AFTER: std::time::Duration = std::time::Duration::from_secs(5);

/// The level 0 to 1 as the meter draws it: a loudness curve (the raw value is amplitude), so quiet speech still moves it.
pub fn meter_position(level: f32) -> f32 { (level.max(0.).sqrt() * 1.6).min(1.) }

/// The sound server's own name for the source whose description is `desc` (the name the device list shows). `None`: the
/// system default, or no `pactl`.
pub fn pulse_source_for(desc: Option<&str>) -> Option<String> {
    let desc = desc?;
    let o = std::process::Command::new("pactl").args(["list", "sources"]).stderr(std::process::Stdio::null()).output().ok()?;
    let t = String::from_utf8_lossy(&o.stdout);
    let mut name: Option<String> = None;
    for l in t.lines() {
        let l = l.trim();
        if let Some(n) = l.strip_prefix("Name: ") { name = Some(n.to_string()); }
        else if let Some(d) = l.strip_prefix("Description: ") { if d == desc && !name.as_deref().unwrap_or("").ends_with(".monitor") { return name; } }
    }
    None
}

/// A `parec` recording of one source. A thread reads it in small chunks and keeps the newest level; dropping this ends
/// the recording.
pub struct Capture { child: std::process::Child, level: std::sync::Arc<std::sync::atomic::AtomicU32>, pub source: Option<String> }

impl Capture {
    pub fn start(source: Option<String>) -> Option<Capture> {
        let mut c = std::process::Command::new("parec");
        c.args(["--format=s16le", "--rate=16000", "--channels=1", "--latency-msec=50", "--client-name=mega-mic-test"]);
        if let Some(s) = &source { c.arg(format!("--device={s}")); }
        c.stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).stdout(std::process::Stdio::piped());
        let mut child = c.spawn().ok()?;
        let mut out = child.stdout.take()?;
        let level = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let l2 = level.clone();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = [0u8; 1600];
            while let Ok(n) = out.read(&mut buf) {
                if n < 2 { break; }
                let s: Vec<i16> = buf[..n - n % 2].chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
                l2.store(super::audio::rms_i16(&s).to_bits(), std::sync::atomic::Ordering::Relaxed);
            }
        });
        Some(Capture { child, level, source })
    }
    /// The newest chunk's level.
    pub fn level(&self) -> f32 { f32::from_bits(self.level.load(std::sync::atomic::Ordering::Relaxed)) }
    pub fn alive(&mut self) -> bool { matches!(self.child.try_wait(), Ok(None)) }
}

impl Drop for Capture {
    fn drop(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_meter_curve_moves_for_quiet_speech_and_stops_at_one() {
        assert_eq!(meter_position(0.), 0.);
        assert!(meter_position(0.01) > 0.15, "quiet speech is visible");
        assert_eq!(meter_position(1.), 1.);
        assert!(meter_position(SILENT) < 0.03, "silence is flat");
    }
}
