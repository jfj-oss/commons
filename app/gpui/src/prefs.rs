//! What this window remembers for itself, the way the web UI keeps a few choices in localStorage: a small JSON file
//! beside the account (never inside the core's own files). Unknown or missing values fall back to the defaults.
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Saver { Auto, On, Off }
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ChatFont { Bellefair, Raster }

#[derive(Clone, Debug)]
pub struct Prefs { pub saver: Saver, pub chat_font: ChatFont, pub presence: bool, path: PathBuf }

impl Prefs {
    pub fn load(dir: &Path) -> Prefs {
        let path = dir.join("gpui-prefs.json");
        let v: Value = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
        Prefs {
            saver: match v["saver"].as_str() { Some("on") => Saver::On, Some("off") => Saver::Off, _ => Saver::Auto },
            chat_font: if v["chat_font"].as_str() == Some("raster") { ChatFont::Raster } else { ChatFont::Bellefair },
            presence: v["presence"].as_bool() != Some(false),
            path,
        }
    }
    pub fn save(&self) {
        let v = json!({
            "saver": match self.saver { Saver::Auto => "auto", Saver::On => "on", Saver::Off => "off" },
            "chat_font": match self.chat_font { ChatFont::Bellefair => "bellefair", ChatFont::Raster => "raster" },
            "presence": self.presence,
        });
        // the folder exists once there is an account; before that there is nothing worth remembering
        if self.path.parent().is_some_and(|d| d.is_dir()) { let _ = std::fs::write(&self.path, serde_json::to_vec_pretty(&v).unwrap_or_default()); }
    }
}
