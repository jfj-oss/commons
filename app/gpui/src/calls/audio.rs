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
    Ok(Devices {
        mic: p.recording_devices().map(|d| AudioDevice { id: d.id.as_str().to_string(), name: d.name, index: d.index }).collect(),
        out: p.playout_devices().map(|d| AudioDevice { id: d.id.as_str().to_string(), name: d.name, index: d.index }).collect(),
    })
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

    #[test]
    fn processing_round_trips_through_the_sdk_options() {
        let p = Processing { echo_cancellation: true, noise_suppression: false, auto_gain_control: true };
        let o: AudioSourceOptions = p.into();
        assert_eq!(o.echo_cancellation, true);
        assert_eq!(o.noise_suppression, false);
        assert_eq!(Processing::from(o), p);
    }
}
