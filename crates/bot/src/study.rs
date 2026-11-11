//! Due reminders from the owner's study planner (homebase `study/`, port 4575), posted into one room.  STUB: the design and
//! the pure parts are here and tested; the HTTP calls and the thread in `main.rs` are written but not wired in (see
//! `STUDY-REMINDERS.md` beside the crate).  Nothing here runs unless `MEGATRON_BOT_STUDY_ROOM` is set.
//!
//! How it works, in one paragraph.  The planner makes the schedule (a week before, two days before and the morning of each
//! piece of graded work, with per-course mutes, all from the syllabus tables, no model).  Every minute this module asks
//! `GET /api/reminders/due?since=<when it last asked>` with the shared homebase token; the planner answers with the
//! reminders whose time has come and that were not posted yet.  The bot posts each one as a plain message in the chosen
//! room and then tells the planner `POST /api/reminders/sent {ids}`, so the planner, not the bot, remembers what went out:
//! a bot restart never repeats a reminder, and a planner that was down posts nothing stale (it drops reminders older than
//! 36 hours by itself).  The planner is only ever read; the bot never changes a date, a weight or a mute.
//!
//! Why a poll and not a push: the planner runs in a tight systemd sandbox with its own token, and the bot already keeps a
//! client open to the relay.  A GET every minute from the bot is one more loopback request; a push would need the planner
//! to hold mega keys.  Why not a file: the bot's data dir is private to it, and the planner's sandbox cannot write there.
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

pub const DEFAULT_STUDY_URL: &str = "http://127.0.0.1:4575";
pub const POLL_EVERY: Duration = Duration::from_secs(60);
pub const API_TIMEOUT: Duration = Duration::from_secs(10);
/// A reminder's text is short; anything longer is cut, in case the planner's data were ever odd.
pub const MAX_TEXT_CHARS: usize = 400;

/// One reminder as the planner lists it (`study/reminders.py`, `schedule`).  Only the fields the bot needs.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Reminder {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub course: String,
    #[serde(default)]
    pub at: String,
}

#[derive(Debug, Deserialize)]
struct DueReply {
    ok: bool,
    #[serde(default)]
    now: String,
    #[serde(default)]
    reminders: Vec<Reminder>,
}

/// The planner's answer to `/api/reminders/due`, parsed and checked.  `now` is the planner's clock, which becomes the next `since`.
pub fn parse_due(body: &str) -> Result<(String, Vec<Reminder>), String> {
    let reply: DueReply = serde_json::from_str(body).map_err(|e| format!("planner answer is not the expected JSON: {e}"))?;
    if !reply.ok { return Err("planner said not ok".into()); }
    let mut out = Vec::new();
    for r in reply.reminders {
        if r.id.is_empty() || r.text.trim().is_empty() { continue; }
        if !r.id.chars().all(|c| c.is_ascii_digit() || c == ':') { continue; }   // ids are "<graded id>:<days before>"
        out.push(Reminder { text: r.text.chars().take(MAX_TEXT_CHARS).collect::<String>().trim().to_string(), ..r });
    }
    Ok((reply.now, out))
}

/// What the bot posts: one message per poll, never a flood.  Several reminders due at once become one list.
pub fn message(reminders: &[Reminder]) -> Option<String> {
    match reminders {
        [] => None,
        [one] => Some(format!("Reminder: {}", one.text)),
        many => Some(format!("Reminders:\n{}", many.iter().map(|r| format!("- {}", r.text)).collect::<Vec<_>>().join("\n"))),
    }
}

/// The body for `POST /api/reminders/sent`.
pub fn sent_body(reminders: &[Reminder]) -> String {
    serde_json::json!({ "ids": reminders.iter().map(|r| r.id.as_str()).collect::<Vec<_>>() }).to_string()
}

/// Settings from the environment.  `None` means the feature is off (no room was named), which is the default.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub url: String,
    pub room: String,
    pub token_file: std::path::PathBuf,
}
pub fn settings(home: &Path, env: impl Fn(&str) -> Option<String>) -> Option<Settings> {
    let room = env("MEGATRON_BOT_STUDY_ROOM")?;
    Some(Settings {
        url: env("MEGATRON_BOT_STUDY_URL").unwrap_or_else(|| DEFAULT_STUDY_URL.into()),
        room,
        token_file: env("MEGATRON_BOT_STUDY_TOKEN_FILE").map(std::path::PathBuf::from).unwrap_or_else(|| home.join(".config/homebase/token")),
    })
}

/// The planner over HTTP.  Only two routes are ever called.  (Not wired in: see the module note.)
pub trait StudyApi: Send + Sync {
    /// Ask for the reminders due now; `since` is the planner's own `now` from the last answer, or empty the first time.
    fn due(&self, since: &str) -> Result<(String, Vec<Reminder>), String>;
    /// Say these were posted.
    fn sent(&self, reminders: &[Reminder]) -> Result<(), String>;
}

pub struct HttpStudy {
    base: String,
    token: Option<String>,
}
impl HttpStudy {
    pub fn new(base: &str, token_file: &Path) -> HttpStudy {
        let token = std::fs::read_to_string(token_file).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        HttpStudy { base: base.trim_end_matches('/').to_string(), token }
    }
    fn token(&self) -> Result<&str, String> { self.token.as_deref().ok_or_else(|| "the homebase token file is not readable".to_string()) }
}
impl StudyApi for HttpStudy {
    fn due(&self, since: &str) -> Result<(String, Vec<Reminder>), String> {
        let url = if since.is_empty() { format!("{}/api/reminders/due", self.base) } else { format!("{}/api/reminders/due?since={}", self.base, since) };
        let body = ureq::get(&url).set("X-Homebase-Token", self.token()?).timeout(API_TIMEOUT).call()
            .map_err(|e| format!("planner not answering: {e}"))?.into_string().map_err(|e| e.to_string())?;
        parse_due(&body)
    }
    fn sent(&self, reminders: &[Reminder]) -> Result<(), String> {
        if reminders.is_empty() { return Ok(()); }
        ureq::post(&format!("{}/api/reminders/sent", self.base)).set("X-Homebase-Token", self.token()?).set("Content-Type", "application/json")
            .timeout(API_TIMEOUT).send_string(&sent_body(reminders)).map_err(|e| format!("could not record the reminders as sent: {e}"))?;
        Ok(())
    }
}

/// One poll: ask, post, record.  Gives the planner's `now` to keep as the next `since`.  A reminder is recorded as sent only
/// after the room took the message; if the post fails the planner still has it and the next poll tries again.
pub fn poll_once(api: &dyn StudyApi, post: &mut dyn FnMut(&str) -> Result<(), String>, since: &str) -> Result<String, String> {
    let (now, due) = api.due(since)?;
    if let Some(text) = message(&due) {
        post(&text)?;
        api.sent(&due)?;
    }
    Ok(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = r#"{"ok": true, "now": "2026-11-11T08:00", "reminders": [
        {"id": "7:7", "text": "HIST 398: In-Class Essay Exam 2 (25%) is due in a week, Wed Nov 18.", "course": "HIST 398", "at": "2026-11-11T08:00", "weight": 25.0},
        {"id": "12:2", "text": "HIST 398: SE Asia Paper: abstract (1%) is due in 2 days, Fri Nov 13.", "course": "HIST 398", "at": "2026-11-11T08:00"},
        {"id": "bad id", "text": "ignored"}, {"id": "3:0", "text": "   "}]}"#;

    #[test]
    fn parses_and_keeps_only_well_formed_reminders() {
        let (now, due) = parse_due(BODY).unwrap();
        assert_eq!(now, "2026-11-11T08:00");
        assert_eq!(due.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["7:7", "12:2"]);
        assert!(parse_due(r#"{"ok": false}"#).is_err());
        assert!(parse_due("nonsense").is_err());
    }

    #[test]
    fn one_message_per_poll() {
        let (_, due) = parse_due(BODY).unwrap();
        assert_eq!(message(&[]), None);
        assert_eq!(message(&due[..1]).unwrap(), "Reminder: HIST 398: In-Class Essay Exam 2 (25%) is due in a week, Wed Nov 18.");
        assert!(message(&due).unwrap().starts_with("Reminders:\n- HIST 398: In-Class Essay Exam 2"));
        assert_eq!(sent_body(&due), r#"{"ids":["7:7","12:2"]}"#);
    }

    #[test]
    fn off_unless_a_room_is_named() {
        let home = Path::new("/home/x");
        assert_eq!(settings(home, |_| None), None);
        let s = settings(home, |k| if k == "MEGATRON_BOT_STUDY_ROOM" { Some("room-1".into()) } else { None }).unwrap();
        assert_eq!(s, Settings { url: DEFAULT_STUDY_URL.into(), room: "room-1".into(), token_file: home.join(".config/homebase/token") });
    }

    struct Fake { answers: std::cell::RefCell<Vec<(String, Vec<Reminder>)>>, sent: std::cell::RefCell<Vec<String>>, since_seen: std::cell::RefCell<Vec<String>> }
    unsafe impl Sync for Fake {}
    impl StudyApi for Fake {
        fn due(&self, since: &str) -> Result<(String, Vec<Reminder>), String> { self.since_seen.borrow_mut().push(since.into()); Ok(self.answers.borrow_mut().remove(0)) }
        fn sent(&self, r: &[Reminder]) -> Result<(), String> { self.sent.borrow_mut().extend(r.iter().map(|x| x.id.clone())); Ok(()) }
    }

    #[test]
    fn poll_posts_then_records_and_carries_since() {
        let (_, due) = parse_due(BODY).unwrap();
        let fake = Fake { answers: std::cell::RefCell::new(vec![("2026-11-11T08:00".into(), due), ("2026-11-11T08:01".into(), vec![])]), sent: Default::default(), since_seen: Default::default() };
        let mut posted = Vec::new();
        let since = poll_once(&fake, &mut |t| { posted.push(t.to_string()); Ok(()) }, "").unwrap();
        assert_eq!(since, "2026-11-11T08:00");
        assert_eq!(posted.len(), 1);
        assert_eq!(*fake.sent.borrow(), ["7:7", "12:2"]);
        let since = poll_once(&fake, &mut |_| Ok(()), &since).unwrap();
        assert_eq!(since, "2026-11-11T08:01");
        assert_eq!(*fake.since_seen.borrow(), ["", "2026-11-11T08:00"]);
    }

    #[test]
    fn a_failed_post_records_nothing() {
        let (_, due) = parse_due(BODY).unwrap();
        let fake = Fake { answers: std::cell::RefCell::new(vec![("2026-11-11T08:00".into(), due)]), sent: Default::default(), since_seen: Default::default() };
        assert!(poll_once(&fake, &mut |_| Err("room gone".into()), "").is_err());
        assert!(fake.sent.borrow().is_empty());
    }
}
