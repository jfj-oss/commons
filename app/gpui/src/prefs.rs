//! What this window remembers for itself, the way the web UI keeps a few choices in localStorage: a small JSON file
//! beside the account (never inside the core's own files). Unknown or missing values fall back to the defaults.
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Saver { Auto, On, Off }
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ChatFont { Bellefair, Raster }
/// The sound for new messages (the web's Settings choice): every message, mentions and replies only, or none.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sound { All, Mentions, Off }
/// How big the chat text reads on this device.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReadSize { Small, Normal, Large }
impl ReadSize { pub fn scale(self) -> f32 { match self { ReadSize::Small => 0.85, ReadSize::Normal => 1.0, ReadSize::Large => 1.2 } } }

#[derive(Clone, Debug)]
pub struct Prefs { pub saver: Saver, pub chat_font: ChatFont, pub presence: bool, /** the ring sound (off: rings only show) */ pub sound: bool, /** the cameras-and-screens column's width on this device, 0 = the default */ pub deck_w: f32, /** the chime for new messages */ pub msg_sound: Sound, /** the chat text's size */ pub read_size: ReadSize, path: PathBuf }

impl Prefs {
    pub fn load(dir: &Path) -> Prefs {
        let path = dir.join("gpui-prefs.json");
        let v: Value = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
        Prefs {
            saver: match v["saver"].as_str() { Some("on") => Saver::On, Some("off") => Saver::Off, _ => Saver::Auto },
            chat_font: if v["chat_font"].as_str() == Some("raster") { ChatFont::Raster } else { ChatFont::Bellefair },
            presence: v["presence"].as_bool() != Some(false),
            sound: v["sound"].as_bool() != Some(false),
            msg_sound: match v["msg_sound"].as_str() { Some("mentions") => Sound::Mentions, Some("off") => Sound::Off, _ => Sound::All },
            read_size: match v["read_size"].as_str() { Some("small") => ReadSize::Small, Some("large") => ReadSize::Large, _ => ReadSize::Normal },
            deck_w: v["deck_w"].as_f64().filter(|w| *w > 0. && *w < 20000.).unwrap_or(0.) as f32,
            path,
        }
    }
    pub fn save(&self) {
        let v = json!({
            "saver": match self.saver { Saver::Auto => "auto", Saver::On => "on", Saver::Off => "off" },
            "chat_font": match self.chat_font { ChatFont::Bellefair => "bellefair", ChatFont::Raster => "raster" },
            "presence": self.presence,
            "sound": self.sound,
            "msg_sound": match self.msg_sound { Sound::All => "all", Sound::Mentions => "mentions", Sound::Off => "off" },
            "read_size": match self.read_size { ReadSize::Small => "small", ReadSize::Normal => "normal", ReadSize::Large => "large" },
            "deck_w": self.deck_w,
        });
        // the folder exists once there is an account; before that there is nothing worth remembering
        if self.path.parent().is_some_and(|d| d.is_dir()) { let _ = std::fs::write(&self.path, serde_json::to_vec_pretty(&v).unwrap_or_default()); }
    }
}
