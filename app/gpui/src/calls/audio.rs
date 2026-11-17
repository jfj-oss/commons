//! Platform audio (CALLS-PLAN.md step 3): the device lists, the three processing switches, and the per-person
//! playout measure. Devices and processing go through the LiveKit SDK's `PlatformAudio`, which drives libwebrtc's own
//! audio device module (PulseAudio on Linux, CoreAudio on macOS). Nothing here opens a stream on its own: listing
//! devices does not record or play, and the microphone is only opened when a track is published (engine.rs).
use livekit::prelude::*;
use livekit::webrtc::audio_source::AudioSourceOptions;
use serde::Serialize;

/// The three software audio processing switches, the native counterpart of `echoCancellation`, `noiseSuppression`
/// and `autoGainControl` in call.js's prefs. libwebrtc applies them to the microphone source; they are baked in when
/// the source is created, so changing one mid-call republishes the microphone track (see `MicWish` in engine.rs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Processing {
    pub echo_cancellation: bool,
    pub noise_suppression: bool,
    pub auto_gain_control: bool,
}

impl Processing {
    /// The default: all three on, exactly as call.js's prefs start.
    pub const ON: Processing = Processing { echo_cancellation: true, noise_suppression: true, auto_gain_control: true };

    /// call.js `setNoiseSuppression`: turning noise suppression on also turns echo cancellation on (never the other way
    /// round).
    pub fn with_noise(self, on: bool) -> Processing {
        Processing { noise_suppression: on, echo_cancellation: self.echo_cancellation || on, ..self }
    }
}

impl Default for Processing {
    fn default() -> Processing { Processing::ON }
}

impl From<Processing> for AudioSourceOptions {
    fn from(p: Processing) -> AudioSourceOptions {
        AudioSourceOptions { echo_cancellation: p.echo_cancellation, noise_suppression: p.noise_suppression, auto_gain_control: p.auto_gain_control }
    }
}

impl From<AudioSourceOptions> for Processing {
    fn from(o: AudioSourceOptions) -> Processing {
        Processing { echo_cancellation: o.echo_cancellation, noise_suppression: o.noise_suppression, auto_gain_control: o.auto_gain_control }
    }
}

/// One audio device, whatever the platform calls it. `id` is the SDK's stable identifier (a GUID, or `index:N` when
/// the backend has no GUID); the settings file stores it and an id that has gone away falls back to the default.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct AudioDevice { pub id: String, pub name: String, pub index: usize }

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Devices {
    pub mic: Vec<AudioDevice>,
    pub out: Vec<AudioDevice>,
}

/// Lists the platform's microphones (`mic`) and speakers (`out`). A temporary `PlatformAudio` is made when the engine
/// does not hold one (for the settings picker, before a call); it is dropped right after, and listing never opens a
/// stream, so nothing is recorded or played.
pub fn list_devices() -> Result<Devices, String> {
    let p = PlatformAudio::new().map_err(|e| e.to_string())?;
    Ok(lists(&p))
}

/// The two lists of an existing `PlatformAudio`.
pub fn lists(p: &PlatformAudio) -> Devices {
    Devices {
        mic: p.recording_devices().map(|d| AudioDevice { id: d.id.as_str().to_string(), name: d.name, index: d.index }).collect(),
        out: p.playout_devices().map(|d| AudioDevice { id: d.id.as_str().to_string(), name: d.name, index: d.index }).collect(),
    }
}

/// Is this the platform's "whatever is the system default" entry? On PulseAudio and PipeWire that is index 0, named
/// `default: <description>`; there is no such entry on every backend.
pub fn is_default_entry(d: &AudioDevice) -> bool { d.name.starts_with("default:") || d.name == "default" }

/// Which device a remembered choice means, now. The PulseAudio backend has no GUIDs, so an id is `index:N` and N moves
/// whenever a device is plugged in or out: only the NAME is a stable handle there. Order of trust: the remembered name
/// (exact match, never the "default:" entry), then a remembered real GUID. A bare `index:N` from an older settings file
/// is not trusted (it may be a different device by now), and anything not found means the system default: `None`.
pub fn resolve(devs: &[AudioDevice], id: Option<&str>, name: Option<&str>) -> Option<String> {
    let name = name.filter(|n| !n.is_empty());
    if let Some(n) = name { return devs.iter().find(|d| !is_default_entry(d) && d.name == n).map(|d| d.id.clone()); }
    let id = id.filter(|i| !i.is_empty() && !i.starts_with("index:"))?;
    devs.iter().find(|d| d.id == id).map(|d| d.id.clone())
}

/// The id a device must be switched to for "system default": the default entry if the backend has one.
pub fn default_id(devs: &[AudioDevice]) -> Option<String> { devs.iter().find(|d| is_default_entry(d)).map(|d| d.id.clone()) }

/// RMS of signed 16-bit samples, 0 to 1.
pub fn rms_i16(s: &[i16]) -> f32 {
    if s.is_empty() { return 0.; }
    (s.iter().map(|&x| { let v = x as f64 / 32768.; v * v }).sum::<f64>() / s.len() as f64).sqrt() as f32
}

/// Is the sound server's default (or named) microphone switched off in the system's own settings? Linux only
/// (`pactl`); `None` where it cannot be told. A muted system microphone delivers digital silence to every app, and
/// the app cannot tell that from a quiet room unless it asks.
pub fn system_mic_muted() -> Option<bool> {
    #[cfg(target_os = "linux")]
    {
        let o = std::process::Command::new("pactl").args(["get-source-mute", "@DEFAULT_SOURCE@"]).stderr(std::process::Stdio::null()).output().ok()?;
        let t = String::from_utf8_lossy(&o.stdout);
        if t.contains("yes") { return Some(true); }
        if t.contains("no") { return Some(false); }
    }
    None
}

/// The SDK's id type for a recording device, rebuilt from the string the settings file (or the UI) stored.
pub fn recording_id(id: &str) -> RecordingDeviceId { RecordingDeviceId::from_unchecked_guid(id) }
pub fn playout_id(id: &str) -> PlayoutDeviceId { PlayoutDeviceId::from_unchecked_guid(id) }

/// The receiver's own playout energy for one remote microphone: `inbound-rtp`'s `totalAudioEnergy`, which libwebrtc
/// takes in the playout path after patch W1's gain. This is what a per-person volume of 0 actually moves; a
/// `NativeAudioStream` tap sits before the gain and does not show it (CALLS-PLAN.md "Corrections").
pub async fn playout_energy(t: &RemoteAudioTrack) -> f64 {
    t.get_stats().await.unwrap_or_default().into_iter().find_map(|s| match s {
        livekit::webrtc::stats::RtcStats::InboundRtp(r) => Some(r.inbound.total_audio_energy),
        _ => None,
    }).unwrap_or(-1.)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processing_defaults_on_and_noise_forces_echo() {
        let p = Processing::default();
        assert!(p.echo_cancellation && p.noise_suppression && p.auto_gain_control);
        let off = p.with_noise(false);
        assert!(!off.noise_suppression && off.echo_cancellation, "turning noise off leaves echo as it was");
        let on = Processing { echo_cancellation: false, noise_suppression: false, auto_gain_control: true }.with_noise(true);
        assert!(on.noise_suppression && on.echo_cancellation, "turning noise on forces echo on");
    }

    fn dev(i: usize, n: &str) -> AudioDevice { AudioDevice { id: format!("index:{i}"), name: n.into(), index: i } }
    fn list() -> Vec<AudioDevice> { vec![dev(0, "default: Q1U"), dev(1, "Q1U"), dev(2, "Webcam mic")] }

    #[test]
    fn a_remembered_name_finds_its_device_wherever_it_moved() {
        let mut l = list();
        assert_eq!(resolve(&l, None, Some("Webcam mic")).as_deref(), Some("index:2"));
        l = vec![dev(0, "default: Webcam mic"), dev(1, "Webcam mic")];   // the Q1U was unplugged: the webcam moves to index 1
        assert_eq!(resolve(&l, Some("index:2"), Some("Webcam mic")).as_deref(), Some("index:1"), "the name wins over the stale index");
        assert_eq!(resolve(&l, None, Some("Q1U")), None, "a device that is gone falls back to the system default");
        assert_eq!(resolve(&l, None, Some("default: Q1U")), None, "the default entry is never picked by name");
    }

    #[test]
    fn an_old_index_id_is_not_trusted_but_a_real_guid_is() {
        let l = list();
        assert_eq!(resolve(&l, Some("index:1"), None), None, "the user's old micId: index:1 means the system default now");
        assert_eq!(resolve(&l, Some(""), None), None);
        let g = vec![AudioDevice { id: "{abc}".into(), name: "USB".into(), index: 0 }];
        assert_eq!(resolve(&g, Some("{abc}"), None).as_deref(), Some("{abc}"));
        assert_eq!(resolve(&g, Some("{gone}"), None), None);
        assert_eq!(default_id(&l).as_deref(), Some("index:0"));
    }

    #[test]
    fn rms_of_silence_and_of_a_loud_wave() {
        assert_eq!(rms_i16(&[0; 100]), 0.);
        assert!((rms_i16(&[16384, -16384, 16384, -16384]) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn processing_round_trips_through_the_sdk_options() {
        let p = Processing { echo_cancellation: true, noise_suppression: false, auto_gain_control: true };
        let o: AudioSourceOptions = p.into();
        assert_eq!(o.echo_cancellation, true);
        assert_eq!(o.noise_suppression, false);
        assert_eq!(Processing::from(o), p);
    }
}
