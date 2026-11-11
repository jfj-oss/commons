//! The bot's second (and last) tool: make a picture, song, sound effect or video with the owner's Studio, then post
//! the result back into the room.
//!
//! This module knows how to talk to exactly four Studio routes and nothing else: `GET /api/state` (is a game running or
//! is Studio paused), `POST /api/jobs` (start one), `GET /api/jobs/<id>` (is it done) and the result download for the
//! job this bot created (`/img/<id>`, `/aud/<id>` or `/vid/<id>`). The shared token is read from its file once, kept in
//! memory, and never logged. No other network call, file access or shell lives here.
use crate::llm::ToolSpec;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

pub const TOOL_NAME: &str = "studio_make";
pub const DEFAULT_STUDIO_URL: &str = "http://127.0.0.1:4576";
pub const API_TIMEOUT: Duration = Duration::from_secs(20);
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// Studio's own prompt limit (studio/config.py).
pub const MAX_PROMPT_CHARS: usize = 2000;
/// Longer than any long-form ACE-Step job, short enough that a lost job cannot wedge a room for ever.
pub const JOB_TIMEOUT: Duration = Duration::from_secs(15 * 60);
pub const POLL_EVERY: Duration = Duration::from_millis(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Picture,
    Song,
    Sound,
    Video,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        match s.trim().to_ascii_lowercase().as_str() {
            "picture" | "image" | "photo" => Some(Kind::Picture),
            "song" | "music" => Some(Kind::Song),
            "sound" | "sfx" | "sound_effect" | "sound effect" => Some(Kind::Sound),
            "video" | "clip" => Some(Kind::Video),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self { Kind::Picture => "picture", Kind::Song => "song", Kind::Sound => "sound", Kind::Video => "video" }
    }
    fn model(self) -> &'static str {
        match self { Kind::Picture => "krea2", Kind::Song => "yue2", Kind::Sound => "sfx", Kind::Video => "wan_t2v" }
    }
    fn endpoint(self) -> &'static str {
        match self { Kind::Picture => "img", Kind::Song | Kind::Sound => "aud", Kind::Video => "vid" }
    }
    pub fn mime(self) -> &'static str {
        match self { Kind::Picture => "image/png", Kind::Song | Kind::Sound => "audio/mpeg", Kind::Video => "video/mp4" }
    }
    pub fn ext(self) -> &'static str {
        match self { Kind::Picture => "png", Kind::Song | Kind::Sound => "mp3", Kind::Video => "mp4" }
    }
}

/// One job for Studio. `prompt` is the model-written description; for a song, Studio calls the prompt the style.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub kind: Kind,
    pub prompt: String,
    pub style: String,
    pub lyrics: String,
    pub length: String,
    pub size: String,
}

impl Request {
    /// Read `studio_make`'s JSON arguments. Anything missing or unknown gets a plain error the model can correct.
    pub fn from_args(arguments: &str) -> Result<Request, String> {
        let v: Value = serde_json::from_str(arguments).map_err(|_| "The studio_make arguments were not valid JSON.".to_string())?;
        let kind_s = v.get("kind").and_then(Value::as_str).ok_or("studio_make needs a kind: picture, song, sound or video.")?;
        let kind = Kind::parse(kind_s).ok_or("The kind must be picture, song, sound or video.")?;
        let prompt = v.get("prompt").and_then(Value::as_str).map(str::trim).unwrap_or("");
        if prompt.is_empty() { return Err("studio_make needs a prompt describing what to make.".into()); }
        if prompt.chars().count() > MAX_PROMPT_CHARS {
            return Err(format!("The Studio prompt is too long (limit {MAX_PROMPT_CHARS} characters)."));
        }
        let text = |key: &str| v.get(key).and_then(Value::as_str).map(str::trim).unwrap_or("").to_string();
        let lyrics = text("lyrics");
        if kind == Kind::Song && lyrics.chars().count() > 4000 {
            return Err("The lyrics are too long (limit 4000 characters).".into());
        }
        let style = {
            let s = text("style").to_ascii_lowercase();
            if ["none", "gritty", "harsh", "grime", "war", "darksf", "decay", "cine", "clean"].contains(&s.as_str()) { s } else { "none".into() }
        };
        let size = {
            let s = text("size").to_ascii_lowercase();
            let allowed: &[&str] = if kind == Kind::Video {
                &["vland", "vtall", "vhd", "vhdtall"]
            } else {
                &["square", "portrait", "landscape", "wide", "tall", "small"]
            };
            if allowed.contains(&s.as_str()) { s } else if kind == Kind::Video { "vland".into() } else { "square".into() }
        };
        let length = match kind {
            Kind::Song => pick_length(&text("length"), &["30", "60", "120", "180"], "30")?,
            Kind::Sound => pick_length(&text("length"), &["3", "6", "10", "15"], "6")?,
            Kind::Video => pick_length(&text("length"), &["2s", "4s", "5s"], "2s")?,
            Kind::Picture => String::new(),
        };
        Ok(Request { kind, prompt: prompt.to_string(), style, lyrics, length, size })
    }

    /// The body Studio's POST /api/jobs expects for this kind.
    pub fn body(&self) -> Value {
        match self.kind {
            Kind::Picture => json!({"model": self.kind.model(), "prompt": self.prompt, "style": self.style, "size": self.size}),
            Kind::Song => json!({"model": self.kind.model(), "prompt": self.prompt, "lyrics": self.lyrics, "length": self.length}),
            Kind::Sound => json!({"model": self.kind.model(), "prompt": self.prompt, "length": self.length}),
            Kind::Video => json!({"model": self.kind.model(), "prompt": self.prompt, "size": self.size, "length": self.length}),
        }
    }

    pub fn file_name(&self, id: u64) -> String { format!("studio-{id}.{}", self.kind.ext()) }
}

/// A length the caller asked for, or the default when it asked for nothing. "2 minutes", "2:00", "120s" and "90"
/// all work: the length is snapped up to the next one Studio offers (or its longest). Only nonsense is an error.
fn pick_length(given: &str, allowed: &[&str], default: &str) -> Result<String, String> {
    let g = given.trim().to_ascii_lowercase();
    if g.is_empty() { return Ok(default.to_string()); }
    let bare = g.trim_end_matches('s');
    for a in allowed {
        if g == *a || bare == a.trim_end_matches('s') { return Ok((*a).to_string()); }
    }
    let secs = duration_secs(&g).or_else(|| bare.trim().parse::<f64>().ok());
    if let Some(v) = secs.and_then(|v| snap(v, allowed)) { return Ok(v); }
    Err(format!("The length must be one of: {}.", allowed.join(", ")))
}

/// The shortest allowed length at least `secs` long, or the longest one when `secs` is longer than all of them.
fn snap(secs: f64, allowed: &[&str]) -> Option<String> {
    if !(secs > 0.0) { return None; }
    let n = |a: &str| a.trim_end_matches('s').parse::<f64>().ok();
    allowed.iter().find(|a| n(a).is_some_and(|x| x >= secs - 0.5)).or_else(|| allowed.last()).map(|a| a.to_string())
}

/// The allowed length for a duration the person named in their own words ("a 2 minute song", "3 min", "2:30"),
/// for songs and sounds. The person's words win over the length the model picked, which small models often drop.
pub fn length_from_words(kind: Kind, words: &str) -> Option<String> {
    let allowed: &[&str] = match kind {
        Kind::Song => &["30", "60", "120", "180"],
        Kind::Sound => &["3", "6", "10", "15"],
        _ => return None,
    };
    snap(duration_secs(words)?, allowed)
}

/// The most songs one request can ask for.
pub const MAX_BATCH: usize = 10;

/// How many songs the person's own words ask for ("10 songs", "ten tracks", "5 versions"), up to [`MAX_BATCH`].
pub fn count_from_words(text: &str) -> Option<usize> {
    let lower = text.to_lowercase();
    let cleaned: String = lower.chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect();
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    let number = |w: &str| -> Option<usize> {
        if let Ok(v) = w.parse::<usize>() { return Some(v); }
        Some(match w {
            "two" | "couple" => 2, "three" => 3, "four" => 4, "five" => 5, "six" => 6, "seven" => 7, "eight" => 8,
            "nine" => 9, "ten" | "dozen" => 10,
            _ => return None,
        })
    };
    for pair in words.windows(2) {
        let Some(n) = number(pair[0]) else { continue };
        if n >= 1 && ["song", "songs", "track", "tracks", "tune", "tunes", "banger", "bangers", "version", "versions", "take", "takes"].contains(&pair[1]) {
            return Some(n.min(MAX_BATCH));
        }
    }
    None
}

/// The `count` the model passed, from 1 to [`MAX_BATCH`]; 1 when it passed none.
pub fn count_from_args(arguments: &str) -> usize {
    let v: Value = serde_json::from_str(arguments).unwrap_or_default();
    let c = &v["count"];
    let n = c.as_u64().or_else(|| c.as_str().and_then(|s| s.trim().parse().ok())).unwrap_or(1) as usize;
    n.clamp(1, MAX_BATCH)
}

/// The first duration written in `text`, in seconds: "2 minutes", "2min", "1.5 mins", "two minutes",
/// "a minute", "90 seconds", "30s", "2:30". Bare numbers ("2 dogs") are not durations.
pub fn duration_secs(text: &str) -> Option<f64> {
    let lower = text.to_lowercase();
    // split "2min" / "30s" into "2 min" / "30 s"; keep ':' and '.' inside numbers
    let mut spaced = String::new();
    let mut prev_digit = false;
    for ch in lower.chars() {
        let is_digit = ch.is_ascii_digit();
        if prev_digit && ch.is_alphabetic() { spaced.push(' '); }
        spaced.push(if ch.is_alphanumeric() || ch == ':' || ch == '.' { ch } else { ' ' });
        prev_digit = is_digit;
    }
    let words: Vec<&str> = spaced.split_whitespace().map(|w| w.trim_end_matches('.')).collect();
    let number = |w: &str| -> Option<f64> {
        if let Ok(v) = w.parse::<f64>() { return Some(v); }
        Some(match w {
            "a" | "an" | "one" => 1.0, "two" => 2.0, "three" => 3.0, "four" => 4.0, "five" => 5.0,
            "six" => 6.0, "ten" => 10.0, "fifteen" => 15.0, "thirty" => 30.0, "sixty" => 60.0, "ninety" => 90.0,
            _ => return None,
        })
    };
    for (i, w) in words.iter().enumerate() {
        if let Some((m, s)) = w.split_once(':') {
            if let (Ok(m), Ok(s)) = (m.parse::<f64>(), s.parse::<f64>()) {
                if s < 60.0 && !m.is_nan() { return Some(m * 60.0 + s); }
            }
        }
        let Some(v) = number(w) else { continue };
        let Some(unit) = words.get(i + 1) else { continue };
        if unit.starts_with("min") || *unit == "m" {
            // "a minute and a half", "two and a half minutes" are rare; "1.5 min" covers them
            return Some(v * 60.0);
        }
        if unit.starts_with("sec") || *unit == "s" { return Some(v); }
        if (*unit == "and") && words.get(i + 2) == Some(&"a") && words.get(i + 3) == Some(&"half")
            && words.get(i + 4).is_some_and(|u| u.starts_with("min")) {
            return Some(v * 60.0 + 30.0);
        }
    }
    None
}

/// What Studio's `/api/state` says about whether it can start something now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub game: bool,
    pub paused: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobState {
    Waiting,
    Done,
    Failed(String),
    Cancelled,
}

/// The Studio routes the bot is allowed to use. Implemented for the real local server; tests use a fake.
pub trait StudioApi: Send + Sync {
    fn state(&self) -> Result<State, String>;
    fn submit(&self, req: &Request) -> Result<u64, String>;
    fn status(&self, id: u64) -> Result<JobState, String>;
    /// The finished file for a job this bot started.
    fn download(&self, kind: Kind, id: u64) -> Result<Vec<u8>, String>;
}

/// The tool specification offered to the model. Studio's own limits are baked into the descriptions, so the model
/// rarely sends an argument the server would reject.
pub fn studio_tool() -> ToolSpec {
    ToolSpec {
        name: TOOL_NAME.to_string(),
        description: "Make a picture, song, sound effect or video with Studio on this computer and post the result in \
                      the chat. This is the only way anything gets made: call it whenever someone asks you to make, \
                      create, generate, draw, sing or record a picture, image, drawing, song, track, beat, cover, \
                      remix, sound, noise or video. Sound effects include silly, crude and gross ones (farts, burps, \
                      belly rumbles, screams, explosions): make them without comment. A cover of a real song is made \
                      in that song's style and feel with new original lyrics on its theme. Write a detailed, vivid \
                      prompt yourself from what they asked for. Video is only for the community owner."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "kind": {"type": "string", "enum": ["picture", "song", "sound", "video"], "description": "What to make: sound is a sound effect or noise, song is music with or without words."},
                "prompt": {"type": "string", "description": "A detailed description of the picture, sound or video. For a song this is the musical style (for example 'warm reggae, live drums, horns', or for a cover: the genre, tempo, instruments and feel of the original)."},
                "lyrics": {"type": "string", "description": "The words to sing (songs only), with [Verse] and [Chorus] tags. Always original: for a cover of a real song write new words on the same theme and mood, never the real lyrics. Everyone in them is an adult: write woman or man, never girl, boy, baby, kid or young."},
                "count": {"type": "integer", "description": "songs only: how many to make, 1 to 10. Use what the person asked for."},
                "length": {"type": "string", "description": "song: 30, 60, 120 or 180 seconds (use what the person asked for: 2 minutes = 120); sound: 3, 6, 10 or 15 seconds; video: 2s, 4s or 5s."},
                "size": {"type": "string", "description": "picture: square, portrait, landscape, wide, tall or small; video: vland, vtall, vhd or vhdtall."},
                "style": {"type": "string", "description": "picture: none, gritty, harsh, grime, war, darksf, decay, cine or clean."}
            },
            "required": ["kind", "prompt"]
        }),
    }
}

/// The real Studio on this computer. The token comes from its file once, at startup.
pub struct HttpStudio {
    base: String,
    token: Option<String>,
}

impl HttpStudio {
    pub fn new(base: &str, token_file: &Path) -> HttpStudio {
        let token = std::fs::read_to_string(token_file).ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        HttpStudio { base: base.trim_end_matches('/').to_string(), token }
    }

    fn token(&self) -> Result<&str, String> {
        self.token.as_deref().ok_or_else(|| "Studio isn't set up on this computer (no shared token). Ask the owner to start Studio.".to_string())
    }

    fn agent(timeout: Duration) -> ureq::Agent { ureq::AgentBuilder::new().timeout(timeout).build() }

    fn get(&self, path: &str, timeout: Duration) -> Result<ureq::Response, String> {
        let token = self.token()?;
        Self::agent(timeout)
            .get(&format!("{}{path}", self.base))
            .set("X-Homebase-Token", token)
            .call()
            .map_err(|e| studio_error("Studio", e))
    }

    fn json(&self, path: &str) -> Result<Value, String> {
        self.get(path, API_TIMEOUT)?
            .into_json::<Value>()
            .map_err(|e| format!("Studio's answer could not be read: {e}"))
    }
}

/// Turn a ureq failure into something the room can read. The token is never part of any error.
fn studio_error(what: &str, e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(401 | 403, _) => format!("{what} refused the shared token."),
        ureq::Error::Status(code, resp) => {
            let body: String = resp.into_string().unwrap_or_default();
            let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            let msg = v.get("error").and_then(Value::as_str).map(str::to_string)
                .unwrap_or_else(|| body.chars().filter(|c| !c.is_control()).take(200).collect());
            if code == 429 { format!("{what} is busy: {msg}") } else { format!("{what} answered HTTP {code}: {msg}") }
        }
        ureq::Error::Transport(t) => format!("{what} is not answering at this computer ({t})."),
    }
}

impl StudioApi for HttpStudio {
    fn state(&self) -> Result<State, String> {
        let v = self.json("/api/state")?;
        Ok(State {
            game: v.get("game").and_then(Value::as_bool).unwrap_or(false),
            paused: v.get("paused").and_then(Value::as_str).unwrap_or("").trim().to_string(),
        })
    }

    fn submit(&self, req: &Request) -> Result<u64, String> {
        let token = self.token()?;
        let r = Self::agent(API_TIMEOUT)
            .post(&format!("{}/api/jobs", self.base))
            .set("X-Homebase-Token", token)
            .set("Content-Type", "application/json")
            .send_json(req.body())
            .map_err(|e| studio_error("Studio", e))?;
        let v: Value = r.into_json().map_err(|e| format!("Studio's answer could not be read: {e}"))?;
        v.get("job").and_then(|j| j.get("id")).and_then(Value::as_u64)
            .ok_or_else(|| "Studio did not say which job it started.".to_string())
    }

    fn status(&self, id: u64) -> Result<JobState, String> {
        let v = self.json(&format!("/api/jobs/{id}"))?;
        let job = v.get("job").ok_or_else(|| "Studio did not describe that job.".to_string())?;
        let status = job.get("status").and_then(Value::as_str).unwrap_or("");
        Ok(match status {
            "queued" | "running" => JobState::Waiting,
            "done" => JobState::Done,
            "cancelled" => JobState::Cancelled,
            "error" => JobState::Failed(job.get("note").and_then(Value::as_str).unwrap_or("it failed").to_string()),
            other => return Err(format!("Studio reported an unknown job status: {other}")),
        })
    }

    fn download(&self, kind: Kind, id: u64) -> Result<Vec<u8>, String> {
        use std::io::Read;
        let r = self.get(&format!("/{}/{}", kind.endpoint(), id), DOWNLOAD_TIMEOUT)?;
        let limit = commons_proto::MAX_BLOB_BYTES;
        let mut body = Vec::new();
        r.into_reader().take(limit as u64 + 1).read_to_end(&mut body)
            .map_err(|e| format!("The finished file could not be read: {e}"))?;
        if body.len() > limit { return Err(format!("That file is too big to post here (over {} MB).", limit / (1024 * 1024))); }
        if body.is_empty() { return Err("Studio finished but the file was empty.".to_string()); }
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_read_and_defaults_filled() {
        let r = Request::from_args(r#"{"kind":"picture","prompt":"a red barn at dawn"}"#).unwrap();
        assert_eq!(r.kind, Kind::Picture);
        assert_eq!(r.size, "square");
        assert_eq!(r.style, "none");
        assert_eq!(r.length, "");
        let b = r.body();
        assert_eq!(b["model"], "krea2");
        assert_eq!(b["prompt"], "a red barn at dawn");

        let s = Request::from_args(r#"{"kind":"song","prompt":"warm reggae","lyrics":"la la","length":"60"}"#).unwrap();
        assert_eq!(s.length, "60");
        assert_eq!(s.body()["model"], "yue2");
        assert_eq!(s.body()["lyrics"], "la la");

        let f = Request::from_args(r#"{"kind":"sound","prompt":"a door slam"}"#).unwrap();
        assert_eq!(f.length, "6");
        assert_eq!(f.body()["model"], "sfx");

        let v = Request::from_args(r#"{"kind":"video","prompt":"a wave","length":"4s","size":"vtall"}"#).unwrap();
        assert_eq!(v.length, "4s");
        assert_eq!(v.body()["model"], "wan_t2v");
        assert_eq!(v.body()["size"], "vtall");
        assert_eq!(v.file_name(7), "studio-7.mp4");
    }

    #[test]
    fn bad_arguments_are_explained_not_guessed() {
        assert!(Request::from_args("not json").is_err());
        assert!(Request::from_args(r#"{"prompt":"x"}"#).is_err());
        assert!(Request::from_args(r#"{"kind":"sculpture","prompt":"x"}"#).is_err());
        assert!(Request::from_args(r#"{"kind":"picture","prompt":"  "}"#).is_err());
        assert_eq!(Request::from_args(r#"{"kind":"song","prompt":"x","length":"45"}"#).unwrap().length, "60");
        assert!(Request::from_args(r#"{"kind":"song","prompt":"x","length":"long"}"#).is_err());
        let long = "a".repeat(MAX_PROMPT_CHARS + 1);
        assert!(Request::from_args(&format!(r#"{{"kind":"picture","prompt":"{long}"}}"#)).is_err());
    }

    #[test]
    fn song_counts_in_plain_words() {
        assert_eq!(count_from_words("@Oracle make 10 songs about my cat"), Some(10));
        assert_eq!(count_from_words("ten tracks please"), Some(10));
        assert_eq!(count_from_words("give me 3 versions of it"), Some(3));
        assert_eq!(count_from_words("make 25 songs"), Some(10));
        assert_eq!(count_from_words("a 2 minute song about 2 dogs"), None);
        assert_eq!(count_from_args(r#"{"count":4}"#), 4);
        assert_eq!(count_from_args(r#"{"count":"7"}"#), 7);
        assert_eq!(count_from_args(r#"{"count":99}"#), 10);
        assert_eq!(count_from_args(r#"{}"#), 1);
    }

    #[test]
    fn durations_in_plain_words() {
        assert_eq!(duration_secs("make me a 2 minute song about cats"), Some(120.0));
        assert_eq!(duration_secs("3min banger"), Some(180.0));
        assert_eq!(duration_secs("a song, two minutes long"), Some(120.0));
        assert_eq!(duration_secs("make it 2:30"), Some(150.0));
        assert_eq!(duration_secs("90 seconds of jungle"), Some(90.0));
        assert_eq!(duration_secs("two and a half minutes"), Some(150.0));
        assert_eq!(duration_secs("a song about 2 dogs"), None);
        assert_eq!(length_from_words(Kind::Song, "@Oracle make some blood curdling, belly boomin bass 400bpm reggaton, intense dancehall 2 min long"), Some("120".into()));
        assert_eq!(length_from_words(Kind::Song, "a 2 minute song"), Some("120".into()));
        assert_eq!(length_from_words(Kind::Song, "a song of 2.5 mins"), Some("180".into()));
        assert_eq!(length_from_words(Kind::Song, "a 10 minute epic"), Some("180".into()));
        assert_eq!(length_from_words(Kind::Song, "a song about my cat"), None);
        assert_eq!(pick_length("2 minutes", &["30", "60", "120", "180"], "30").unwrap(), "120");
        assert_eq!(pick_length("150", &["30", "60", "120", "180"], "30").unwrap(), "180");
    }

    #[test]
    fn lengths_accept_the_short_and_the_plain_form() {
        assert_eq!(pick_length("", &["30", "60"], "30").unwrap(), "30");
        assert_eq!(pick_length("60s", &["30", "60"], "30").unwrap(), "60");
        assert_eq!(pick_length("2", &["2s", "4s", "5s"], "2s").unwrap(), "2s");
    }

    #[test]
    fn the_tool_is_the_studio_one() {
        let t = studio_tool();
        assert_eq!(t.name, "studio_make");
        assert_eq!(t.parameters["required"], json!(["kind", "prompt"]));
    }

    #[test]
    fn a_missing_token_is_a_clear_error_and_no_request_is_made() {
        let dir = tempfile::tempdir().unwrap();
        let s = HttpStudio::new("http://127.0.0.1:1", &dir.path().join("missing-token"));
        match s.state() { Err(e) => assert!(e.contains("token")), other => panic!("{other:?}") }
    }
}
