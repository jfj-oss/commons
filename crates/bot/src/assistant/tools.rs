//! The local model's tools in the assistant room. Three tiers:
//!   look   - list/read text files under the allowed roots, system status, web search. No approval.
//!   safe   - a fixed allowlist of small actions (music transport, restart a named unit, open a link or file).
//!   handoff - `do_on_computer`: anything else goes to Claude Code, where every step is approved in chat.
//! Nothing here runs a shell, and no tool takes a command line from the model.
use super::config::{expand_home, AssistConfig};
use super::deny::Deny;
use crate::llm::ToolSpec;
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const LIST_FILES: &str = "list_files";
pub const READ_FILE: &str = "read_file";
pub const SYSTEM_STATUS: &str = "system_status";
pub const RUNE: &str = "rune_control";
pub const RESTART_UNIT: &str = "restart_unit";
pub const OPEN: &str = "open_target";
pub const HANDOFF: &str = "do_on_computer";
pub const WEB_SEARCH: &str = crate::search::TOOL_NAME;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier { Look, Safe, Handoff }

pub fn tier_of(name: &str) -> Option<Tier> {
    match name {
        LIST_FILES | READ_FILE | SYSTEM_STATUS | WEB_SEARCH => Some(Tier::Look),
        RUNE | RESTART_UNIT | OPEN => Some(Tier::Safe),
        HANDOFF => Some(Tier::Handoff),
        _ => None,
    }
}

/// Look tools that put outside text in the model's context (file contents, file names, web results): using one taints the turn.
pub fn taints(name: &str) -> bool { matches!(name, READ_FILE | LIST_FILES | WEB_SEARCH) }

fn spec(name: &str, description: &str, properties: Value, required: &[&str]) -> ToolSpec {
    ToolSpec { name: name.into(), description: description.into(), parameters: json!({"type": "object", "properties": properties, "required": required}) }
}

pub fn tool_specs() -> Vec<ToolSpec> {
    vec![
        crate::search::web_search_tool(),
        spec(LIST_FILES, "List the files and folders in a directory under the allowed folders.", json!({"path": {"type": "string", "description": "Absolute path or ~/..."}}), &["path"]),
        spec(READ_FILE, "Read a text file under the allowed folders (large files are cut off).", json!({"path": {"type": "string"}}), &["path"]),
        spec(SYSTEM_STATUS, "Read-only system status. what = load, memory, disk, gpu, or unit (then give the unit name; it reports its systemd user status).",
             json!({"what": {"type": "string", "enum": ["load", "memory", "disk", "gpu", "unit"]}, "unit": {"type": "string"}}), &["what"]),
        spec(RUNE, "Control the rune music player: play, pause, next or prev.", json!({"action": {"type": "string", "enum": ["play", "pause", "next", "prev"]}}), &["action"]),
        spec(RESTART_UNIT, "Restart one systemd user unit from the owner's short allowed list.", json!({"unit": {"type": "string"}}), &["unit"]),
        spec(OPEN, "Open a web address (http or https) or a file in the desktop's default application.", json!({"target": {"type": "string"}}), &["target"]),
        spec(HANDOFF, "Hand a task that acts on the computer (run commands, edit files, install, build, anything not covered by the other tools) to Claude Code. \
                       Every step it takes is approved by the owner in this chat. Describe the task completely.", json!({"task": {"type": "string"}}), &["task"]),
    ]
}

// --------------------------------------------------------------------- the machine, behind a trait so tests can fake it

pub trait Sys: Send + Sync {
    /// Run a program (no shell) and return its exit code and combined output, cut to a few KB.
    fn run(&self, program: &str, args: &[&str]) -> Result<(i32, String), String>;
    /// POST a JSON command to rune's player API.
    fn rune(&self, url: &str, command: &Value) -> Result<String, String>;
}

pub struct RealSys;
impl Sys for RealSys {
    fn run(&self, program: &str, args: &[&str]) -> Result<(i32, String), String> {
        let mut child = Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().map_err(|e| format!("could not start {program}: {e}"))?;
        let mut out = child.stdout.take();
        let mut err = child.stderr.take();
        let reader = std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(o) = out.as_mut() { let mut b = vec![]; let _ = o.take(65_536).read_to_end(&mut b); s.push_str(&String::from_utf8_lossy(&b)); }
            if let Some(e) = err.as_mut() { let mut b = vec![]; let _ = e.take(8_192).read_to_end(&mut b); s.push_str(&String::from_utf8_lossy(&b)); }
            s
        });
        let start = Instant::now();
        let code = loop {
            match child.try_wait() {
                Ok(Some(st)) => break st.code().unwrap_or(-1),
                Ok(None) if start.elapsed() > Duration::from_secs(15) => { let _ = child.kill(); let _ = child.wait(); return Err(format!("{program} took too long")); }
                Ok(None) => std::thread::sleep(Duration::from_millis(40)),
                Err(e) => return Err(e.to_string()),
            }
        };
        let text = reader.join().unwrap_or_default();
        Ok((code, text.chars().take(4000).collect()))
    }
    fn rune(&self, url: &str, command: &Value) -> Result<String, String> {
        let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(5)).build();
        match agent.post(url).send_json(command.clone()) {
            Ok(r) => Ok(r.into_string().unwrap_or_default().chars().take(300).collect()),
            Err(ureq::Error::Status(c, _)) => Err(format!("rune answered HTTP {c}")),
            Err(e) => Err(format!("rune is not reachable ({e})")),
        }
    }
}

// --------------------------------------------------------------------- look tier

fn arg_str(args: &Value, key: &str) -> Option<String> { args[key].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) }

/// Is this a path the look tools may touch? Inside a root, not protected, not a secret by name. Returns the resolved path.
pub fn allowed_path(cfg: &AssistConfig, deny: &Deny, raw: &str) -> Result<PathBuf, String> {
    let p = expand_home(raw.trim(), &cfg.home);
    if !p.is_absolute() { return Err("Give an absolute path (or one starting with ~/).".into()); }
    if deny.is_denied(&p) { return Err("That path is protected and cannot be accessed.".into()); }
    let real = super::deny::resolve(&p);
    if deny.is_denied(&real) { return Err("That path is protected and cannot be accessed.".into()); }
    let roots: Vec<PathBuf> = cfg.roots.iter().map(|r| super::deny::resolve(r)).collect();
    if !roots.iter().any(|r| real.starts_with(r)) {
        return Err(format!("That is outside the folders I may look in ({}).", cfg.roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join(", ")));
    }
    // every part of the path, not just the file: a secrets folder named as such is skipped too
    if real.components().any(|c| Deny::looks_secret(Path::new(c.as_os_str()))) { return Err("That looks like a secrets file and is not readable.".into()); }
    Ok(real)
}

pub fn list_files(cfg: &AssistConfig, deny: &Deny, raw: &str) -> Result<String, String> {
    let dir = allowed_path(cfg, deny, raw)?;
    let rd = std::fs::read_dir(&dir).map_err(|e| format!("cannot list it: {e}"))?;
    let mut rows: Vec<String> = vec![];
    for e in rd.flatten() {
        let path = e.path();
        if deny.is_denied(&path) || Deny::looks_secret(&path) { continue; }
        let Ok(md) = std::fs::symlink_metadata(&path) else { continue };
        let name = e.file_name().to_string_lossy().to_string();
        rows.push(if md.is_dir() { format!("{name}/") } else if md.file_type().is_symlink() { format!("{name} (link)") } else { format!("{name}  {} bytes", md.len()) });
    }
    rows.sort();
    let total = rows.len();
    rows.truncate(cfg.max_list_entries);
    let mut s = rows.join("\n");
    if total > rows.len() { s.push_str(&format!("\n... and {} more", total - rows.len())); }
    if s.is_empty() { s = "(empty)".into(); }
    Ok(s)
}

pub fn read_file(cfg: &AssistConfig, deny: &Deny, raw: &str) -> Result<String, String> {
    let path = allowed_path(cfg, deny, raw)?;
    let md = std::fs::metadata(&path).map_err(|e| format!("cannot read it: {e}"))?;
    if !md.is_file() { return Err("That is not a regular file.".into()); }
    let f = std::fs::File::open(&path).map_err(|e| format!("cannot read it: {e}"))?;
    let mut buf = vec![];
    f.take(cfg.max_read_bytes as u64).read_to_end(&mut buf).map_err(|e| format!("cannot read it: {e}"))?;
    if buf.contains(&0) { return Err("That is a binary file, not text.".into()); }
    let mut text = String::from_utf8_lossy(&buf).to_string();
    if md.len() as usize > cfg.max_read_bytes { text.push_str(&format!("\n[cut off: {} of {} bytes shown]", buf.len(), md.len())); }
    Ok(text)
}

fn valid_unit(u: &str) -> bool {
    !u.is_empty() && u.len() <= 120 && !u.starts_with('-') && u.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | ':'))
}

fn meminfo() -> String {
    let t = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let pick = |k: &str| t.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)).and_then(|n| n.parse::<u64>().ok()).unwrap_or(0);
    let gb = |kb: u64| format!("{:.1} GB", kb as f64 / 1_048_576.0);
    format!("memory: {} total, {} available, swap {} used of {}", gb(pick("MemTotal:")), gb(pick("MemAvailable:")),
        gb(pick("SwapTotal:").saturating_sub(pick("SwapFree:"))), gb(pick("SwapTotal:")))
}

pub fn system_status(sys: &dyn Sys, args: &Value) -> Result<String, String> {
    match arg_str(args, "what").as_deref() {
        Some("load") => Ok(format!("load average (1, 5, 15 min): {}", std::fs::read_to_string("/proc/loadavg").unwrap_or_default().split_whitespace().take(3).collect::<Vec<_>>().join(" "))),
        Some("memory") => Ok(meminfo()),
        Some("disk") => sys.run("df", &["-h", "-x", "tmpfs", "-x", "devtmpfs", "-x", "squashfs", "-x", "efivarfs"]).map(|(_, o)| o),
        Some("gpu") => sys.run("nvidia-smi", &["--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw", "--format=csv,noheader"]).map(|(_, o)| o),
        Some("unit") => {
            let u = arg_str(args, "unit").ok_or("Name the unit.")?;
            if !valid_unit(&u) { return Err("That is not a valid unit name.".into()); }
            sys.run("systemctl", &["--user", "show", "--no-pager", "-p", "Id,Description,ActiveState,SubState,MainPID,ActiveEnterTimestamp,NRestarts,Result", &u]).map(|(_, o)| o)
        }
        _ => Err("what must be load, memory, disk, gpu or unit.".into()),
    }
}

// --------------------------------------------------------------------- safe tier

/// What a safe action would do, validated but not yet done. `None` plan means refused (the string is the reason).
pub enum Safe {
    Rune(&'static str),
    Restart(String),
    OpenUrl(String),
    OpenFile(PathBuf),
}

impl Safe {
    pub fn describe(&self) -> String {
        match self {
            Safe::Rune(a) => format!("rune: {a}"),
            Safe::Restart(u) => format!("systemctl --user restart {u}"),
            Safe::OpenUrl(u) => format!("xdg-open {u}"),
            Safe::OpenFile(p) => format!("xdg-open {}", p.display()),
        }
    }
}

const NEVER_OPEN: &[&str] = &["desktop", "sh", "bash", "zsh", "fish", "py", "pl", "rb", "run", "appimage", "exe", "bat", "cmd", "jar", "service", "timer", "socket", "bin", "so", "scr", "com", "msi"];

pub fn plan_safe(cfg: &AssistConfig, deny: &Deny, name: &str, args: &Value) -> Result<Safe, String> {
    match name {
        RUNE => match arg_str(args, "action").as_deref() {
            Some("play") => Ok(Safe::Rune("resume")),
            Some("pause") => Ok(Safe::Rune("pause")),
            Some("next") => Ok(Safe::Rune("next")),
            Some("prev") => Ok(Safe::Rune("prev")),
            _ => Err("action must be play, pause, next or prev.".into()),
        },
        RESTART_UNIT => {
            let u = arg_str(args, "unit").ok_or("Name the unit.")?;
            if !valid_unit(&u) { return Err("That is not a valid unit name.".into()); }
            let bare = u.strip_suffix(".service").unwrap_or(&u);
            if cfg.units.iter().any(|a| a == &u || a.strip_suffix(".service").unwrap_or(a) == bare) { Ok(Safe::Restart(u)) }
            else { Err(format!("{u} is not on the list of units I may restart (MEGATRON_BOT_ASSIST_UNITS). I can hand that to Claude Code, where you approve it.")) }
        }
        OPEN => {
            let t = arg_str(args, "target").ok_or("Say what to open.")?;
            let low = t.to_ascii_lowercase();
            if low.starts_with("http://") || low.starts_with("https://") {
                if t.chars().any(|c| c.is_whitespace() || c.is_control()) { return Err("That is not a plain web address.".into()); }
                if deny.mentions_protected(&t).is_some() { return Err("That address mentions a protected place.".into()); }
                return Ok(Safe::OpenUrl(t));
            }
            if low.contains("://") || low.starts_with("mailto:") || low.starts_with("javascript:") { return Err("Only http and https addresses, or files, can be opened.".into()); }
            let p = allowed_path(cfg, deny, &t)?;
            let md = std::fs::metadata(&p).map_err(|e| format!("cannot open it: {e}"))?;
            if !md.is_file() { return Err("That is not a file.".into()); }
            use std::os::unix::fs::PermissionsExt;
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            if md.permissions().mode() & 0o111 != 0 || NEVER_OPEN.contains(&ext.as_str()) { return Err("Executable and script files are not opened this way.".into()); }
            Ok(Safe::OpenFile(p))
        }
        _ => Err("not a safe action".into()),
    }
}

pub fn run_safe(cfg: &AssistConfig, sys: &dyn Sys, plan: &Safe) -> Result<String, String> {
    match plan {
        Safe::Rune(cmd) => sys.rune(&format!("{}/api/player/pc", cfg.rune_url.trim_end_matches('/')), &json!({"type": cmd})).map(|_| format!("rune: {cmd} sent")),
        Safe::Restart(u) => match sys.run("systemctl", &["--user", "restart", u])? {
            (0, _) => Ok(format!("{u} restarted")),
            (c, o) => Err(format!("restart failed (exit {c}): {}", o.trim())),
        },
        Safe::OpenUrl(u) => sys.run("xdg-open", &[u]).map(|_| "opened".to_string()),
        Safe::OpenFile(p) => sys.run("xdg-open", &[&p.to_string_lossy()]).map(|_| "opened".to_string()),
    }
}
