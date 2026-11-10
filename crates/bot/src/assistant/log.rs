//! `assistant.log`: one tab-separated line per tool call and Claude command: time, tier, who/what, the command itself,
//! who approved it, and the result. Message text is never logged, only commands.
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

pub struct AssistLog { path: PathBuf, lock: Mutex<()> }

/// `2026-11-10T15:28:40Z` from epoch seconds.
pub fn iso(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn clean(s: &str, max: usize) -> String {
    let one: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let one = one.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max { one } else { format!("{}...", one.chars().take(max).collect::<String>()) }
}

impl AssistLog {
    pub fn new(data_dir: &std::path::Path) -> AssistLog { AssistLog { path: data_dir.join("assistant.log"), lock: Mutex::new(()) } }
    pub fn path(&self) -> &std::path::Path { &self.path }

    /// `tier`: look, safe, claude, handoff, control. `approved_by`: auto, owner, owner-edited, timeout, owner-denied, blocked.
    pub fn line(&self, tier: &str, command: &str, approved_by: &str, result: &str) {
        let line = format!("{}\t{}\t{}\t{}\t{}\n", iso(commons_proto::now_secs()), clean(tier, 16), clean(command, 600), clean(approved_by, 40), clean(result, 80));
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(dir) = self.path.parent() { let _ = std::fs::create_dir_all(dir); }
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).append(true);
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        if let Ok(mut f) = opts.open(&self.path) { let _ = f.write_all(line.as_bytes()); }
    }
}
