//! Problem reports: when a call fails, the app sends the relay's owner what went wrong (the error, the call's own log,
//! the app version and the OS), so whoever runs the relay can see it without asking the member to copy it out.
//! Nothing from messages, keys or tokens goes in a report: the engine builds it from its error and event log only, and
//! [`scrub`] takes the home folder and the computer's user name out of the text before it leaves.
//!
//! A report that cannot be sent (the relay is the thing that cannot be reached, often) waits in `diag-queue.json`
//! beside the account and goes with the next report, or the next time a call connects. At most [`QUEUE_MAX`] wait.
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const QUEUE_MAX: usize = 20;
pub const QUEUE_FILE: &str = "diag-queue.json";

/// The home folder and the user name, out of every string in the report.
pub fn scrub(v: &mut Value) {
    let home = dirs_home();
    let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).ok().filter(|u| u.len() >= 3);
    scrub_with(v, home.as_deref(), user.as_deref());
}

fn dirs_home() -> Option<String> {
    std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).ok().filter(|h| h.len() > 3)
}

pub fn scrub_with(v: &mut Value, home: Option<&str>, user: Option<&str>) {
    match v {
        Value::String(s) => {
            let mut t = s.clone();
            if let Some(h) = home { t = t.replace(h, "~"); }
            if let Some(u) = user { t = replace_word(&t, u, "<user>"); }
            *s = t;
        }
        Value::Array(a) => a.iter_mut().for_each(|x| scrub_with(x, home, user)),
        Value::Object(o) => o.values_mut().for_each(|x| scrub_with(x, home, user)),
        _ => {}
    }
}

/// `user` replaced where it stands as its own word (a path part, a name), not inside a longer word.
fn replace_word(s: &str, user: &str, with: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    while let Some(i) = rest.find(user) {
        let before = rest[..i].chars().next_back();
        let after = rest[i + user.len()..].chars().next();
        out.push_str(&rest[..i]);
        if word(before) || word(after) { out.push_str(user); } else { out.push_str(with); }
        rest = &rest[i + user.len()..];
    }
    out.push_str(rest);
    out
}

pub fn queue_path(dir: &Path) -> PathBuf { dir.join(QUEUE_FILE) }

pub fn load_queue(dir: &Path) -> Vec<Value> {
    std::fs::read(queue_path(dir)).ok().and_then(|b| serde_json::from_slice::<Vec<Value>>(&b).ok()).unwrap_or_default()
}

pub fn save_queue(dir: &Path, q: &[Value]) {
    let p = queue_path(dir);
    if q.is_empty() { let _ = std::fs::remove_file(&p); return; }
    let start = q.len().saturating_sub(QUEUE_MAX);
    if dir.is_dir() { let _ = std::fs::write(&p, serde_json::to_vec(&q[start..]).unwrap_or_default()); }
}

/// Keep a report small enough for the relay (16 KB): the oldest log lines go first, then the longest strings are cut.
pub fn fit(v: &mut Value, max: usize) {
    let size = |v: &Value| v.to_string().len();
    while size(v) > max {
        match v.get_mut("events").and_then(|e| e.as_array_mut()) { Some(ev) if !ev.is_empty() => { ev.remove(0); } _ => break }
    }
    if size(v) > max {
        if let Some(o) = v.as_object_mut() {
            for x in o.values_mut() { if let Value::String(s) = x { if s.chars().count() > 600 { *s = s.chars().take(600).collect::<String>() + "…"; } } }
        }
    }
    if size(v) > max { *v = serde_json::json!({"truncated": true, "kind": v.get("kind").cloned().unwrap_or_default(), "version": v.get("version").cloned().unwrap_or_default()}); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn scrub_takes_out_the_home_folder_and_user_name() {
        let mut v = json!({"error": "could not open /home/maria/.local/share/x", "events": ["user maria joined", "mariachi plays"], "n": 3});
        scrub_with(&mut v, Some("/home/maria"), Some("maria"));
        assert_eq!(v["error"], "could not open ~/.local/share/x");
        assert_eq!(v["events"][0], "user <user> joined");
        assert_eq!(v["events"][1], "mariachi plays", "a longer word is left alone");
        assert_eq!(v["n"], 3);
    }
    #[test]
    fn fit_drops_old_log_lines_first() {
        let mut v = json!({"kind": "call", "error": "x", "events": (0..2000).map(|i| format!("event number {i}")).collect::<Vec<_>>()});
        fit(&mut v, 4000);
        assert!(v.to_string().len() <= 4000);
        let ev = v["events"].as_array().unwrap();
        assert_eq!(ev.last().unwrap(), "event number 1999", "the newest lines stay");
    }
    #[test]
    fn the_queue_keeps_the_newest() {
        let dir = std::env::temp_dir().join(format!("mega-diag-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let q: Vec<Value> = (0..30).map(|i| json!({"i": i})).collect();
        save_queue(&dir, &q);
        let l = load_queue(&dir);
        assert_eq!(l.len(), QUEUE_MAX);
        assert_eq!(l[0]["i"], 10);
        save_queue(&dir, &[]);
        assert!(load_queue(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
