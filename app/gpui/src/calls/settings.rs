//! What calls remember for this device, the native counterpart of the web UI's `commons.call.prefs` and
//! `commons.call.volumes` (call.js lines ~109 and ~582): the microphone and speaker choices, the three audio
//! processing switches, push-to-talk, and one output volume per account. Kept in a small JSON file beside the
//! account (`gpui-call-prefs.json`), never inside the core's own files. Missing or unknown values fall back to the
//! defaults, so an older file keeps working.
use super::audio::Processing;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub struct CallSettings {
    /// The remembered recording (microphone) device id; `None` means the platform default.
    pub mic_id: Option<String>,
    /// The remembered device's NAME: the stable handle (PulseAudio and PipeWire ids are positions, `index:N`, that move).
    pub mic_name: Option<String>,
    /// The remembered playout (speaker) device id; `None` means the platform default.
    pub out_id: Option<String>,
    pub out_name: Option<String>,
    pub processing: Processing,
    pub push_to_talk: bool,
    /// The camera used last (its id) and the camera profile (`720p` unless `1080p` was chosen), as call.js `camId` and `cameraProfile`.
    pub cam_id: Option<String>,
    pub cam_profile: Option<super::camera::CamProfile>,
    /// One output volume per account, 0 to 10 (1 is unchanged), as `commons.call.volumes`.
    pub volumes: BTreeMap<String, f64>,
}

impl Default for CallSettings {
    fn default() -> CallSettings {
        CallSettings { mic_id: None, mic_name: None, out_id: None, out_name: None, processing: Processing::ON, push_to_talk: false, cam_id: None, cam_profile: None, volumes: BTreeMap::new() }
    }
}

impl CallSettings {
    pub fn load(path: &Path) -> CallSettings {
        let v: serde_json::Value =
            std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(serde_json::Value::Null);
        let mut s = CallSettings::default();
        // An empty id was the old "system default"; a bare `index:N` is a position that moves, so it is only kept next to
        // a name (which then wins, see audio::resolve) and otherwise means the system default.
        let keep = |id: &str, has_name: bool| !id.is_empty() && (has_name || !id.starts_with("index:"));
        s.mic_name = v["micName"].as_str().filter(|n| !n.is_empty()).map(String::from);
        s.out_name = v["outName"].as_str().filter(|n| !n.is_empty()).map(String::from);
        if let Some(id) = v["micId"].as_str().filter(|i| keep(i, s.mic_name.is_some())) { s.mic_id = Some(id.to_string()); }
        if let Some(id) = v["outId"].as_str().filter(|i| keep(i, s.out_name.is_some())) { s.out_id = Some(id.to_string()); }
        // The three switches default to on, so `!= Some(false)` rather than `== Some(true)`.
        s.processing.echo_cancellation = v["echoCancellation"].as_bool() != Some(false);
        s.processing.noise_suppression = v["noiseSuppression"].as_bool() != Some(false);
        s.processing.auto_gain_control = v["autoGainControl"].as_bool() != Some(false);
        s.push_to_talk = v["ptt"].as_bool() == Some(true);
        if let Some(id) = v["camId"].as_str() { s.cam_id = Some(id.to_string()); }
        s.cam_profile = v["cameraProfile"].as_str().and_then(super::camera::CamProfile::parse);
        if let Some(vols) = v["volumes"].as_object() {
            for (k, val) in vols { if let Some(x) = val.as_f64() { s.volumes.insert(k.clone(), x.clamp(0., 10.)); } }
        }
        s
    }

    pub fn save(&self, path: &Path) {
        let mut vols = serde_json::Map::new();
        for (k, v) in &self.volumes { vols.insert(k.clone(), serde_json::json!(v)); }
        let v = serde_json::json!({
            "micId": self.mic_id,
            "micName": self.mic_name,
            "outId": self.out_id,
            "outName": self.out_name,
            "echoCancellation": self.processing.echo_cancellation,
            "noiseSuppression": self.processing.noise_suppression,
            "autoGainControl": self.processing.auto_gain_control,
            "ptt": self.push_to_talk,
            "camId": self.cam_id,
            "cameraProfile": self.cam_profile.map(|p| p.name()),
            "volumes": serde_json::Value::Object(vols),
        });
        // the folder exists once there is an account; before that there is nothing worth remembering
        if path.parent().is_some_and(|d| d.is_dir()) { let _ = std::fs::write(path, serde_json::to_vec_pretty(&v).unwrap_or_default()); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_call_js() {
        let s = CallSettings::default();
        assert!(s.processing.echo_cancellation && s.processing.noise_suppression && s.processing.auto_gain_control);
        assert!(!s.push_to_talk);
        assert_eq!(s.mic_id, None);
        assert!(s.volumes.is_empty());
    }

    #[test]
    fn an_old_or_missing_file_falls_back() {
        let dir = std::env::temp_dir().join(format!("mega-call-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("gpui-call-prefs.json");
        let s = CallSettings::load(&p);
        assert_eq!(s, CallSettings::default());
        std::fs::write(&p, br#"{"noiseSuppression": false, "micId": "tone", "volumes": {"a": 0, "b": 12}}"#).unwrap();
        let s = CallSettings::load(&p);
        assert_eq!(s.mic_id.as_deref(), Some("tone"), "a real id is kept");
        assert!(!s.processing.noise_suppression);
        assert!(s.processing.echo_cancellation, "echo stays on by default");
        assert_eq!(s.volumes["a"], 0.);
        assert_eq!(s.volumes["b"], 10., "volumes clamp to 10");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_old_positional_id_migrates_to_the_system_default() {
        let dir = std::env::temp_dir().join(format!("mega-call-settings-mig-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("gpui-call-prefs.json");
        std::fs::write(&p, br#"{"micId": "index:1", "outId": ""}"#).unwrap();
        let s = CallSettings::load(&p);
        assert_eq!((s.mic_id, s.mic_name, s.out_id), (None, None, None));
        std::fs::write(&p, br#"{"micId": "index:1", "micName": "Q1U"}"#).unwrap();
        let s = CallSettings::load(&p);
        assert_eq!((s.mic_id.as_deref(), s.mic_name.as_deref()), (Some("index:1"), Some("Q1U")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_and_load_round_trips() {
        let dir = std::env::temp_dir().join(format!("mega-call-settings-rt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("gpui-call-prefs.json");
        let mut s = CallSettings::default();
        s.out_id = Some("out0".into());
        s.out_name = Some("Speakers".into());
        s.mic_name = Some("Q1U".into());
        s.processing.noise_suppression = false;
        s.push_to_talk = true;
        s.cam_id = Some("/dev/video2".into());
        s.cam_profile = Some(crate::calls::camera::CamProfile::P1080);
        s.volumes.insert("acc".into(), 0.5);
        s.save(&p);
        assert_eq!(CallSettings::load(&p), s);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
