//! Keeping the local model out of the GPU's way.
//!
//! Ollama holds a model in VRAM for five minutes after its last request. That is fine while the bot is being asked
//! things and wrong the moment a game or Studio wants the card, so a small watcher asks this gate, and unloads the
//! model at once when it says the GPU is busy. The game rule is the owner's Studio rule (same process names from the
//! voice app's settings, the same Steam cgroup scope), plus anything running out of a Steam library's
//! `steamapps/common/` — which is where Proton games such as Deadlock appear. Everything is read straight from /proc
//! and /sys/fs/cgroup; nothing here ever shells out.
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const DEFAULT_GAME_PROCESSES: [&str; 2] = ["dwu", "dwu-classic"];
/// A video-render lock older than this is left over from a crash and is ignored (the same rule Lectures uses).
pub const STUDIO_LOCK_MAX_AGE: Duration = Duration::from_secs(2 * 60 * 60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// Why the bot must not load the model right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pause {
    Game(String),
    Studio,
}

impl Pause {
    /// The short reply a question gets instead of a model answer.
    pub fn reply(&self) -> &'static str {
        match self {
            Pause::Game(_) => "I'm paused while a game is running.",
            Pause::Studio => "I'm paused while Studio is making something.",
        }
    }
    /// A line for the log: the detected game, or the Studio work.
    pub fn reason(&self) -> &str {
        match self {
            Pause::Game(reason) => reason,
            Pause::Studio => "Studio is making something",
        }
    }
}

pub trait GpuCheck: Send + Sync {
    fn blocked(&self) -> Option<Pause>;
}

#[derive(Debug, Clone)]
pub struct GpuGate {
    pub proc_root: PathBuf,
    pub cgroup_root: PathBuf,
    pub game_processes: Vec<String>,
    pub studio_lock: PathBuf,
    pub studio_url: String,
    pub token_file: PathBuf,
    pub comfy_url: String,
    pub probe_timeout: Duration,
}

fn home() -> PathBuf { std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")) }
fn env_path(name: &str, default: PathBuf) -> PathBuf {
    std::env::var_os(name).map(PathBuf::from).unwrap_or(default)
}
fn env_string(name: &str, default: &str) -> String {
    std::env::var(name).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| default.to_string())
}

impl GpuGate {
    /// The real thing: /proc, /sys/fs/cgroup, the voice app's game list, Studio and ComfyUI on this PC.
    pub fn from_env() -> GpuGate {
        let voice = env_path("MEGATRON_BOT_VOICE_SETTINGS", home().join("projects/homebase/voice/data/settings.json"));
        GpuGate {
            proc_root: PathBuf::from("/proc"),
            cgroup_root: PathBuf::from("/sys/fs/cgroup"),
            game_processes: read_game_processes(&voice),
            studio_lock: env_path("MEGATRON_BOT_STUDIO_LOCK", home().join("data/studio/video-render.lock")),
            studio_url: env_string("MEGATRON_BOT_STUDIO_URL", "http://127.0.0.1:4576").trim_end_matches('/').to_string(),
            token_file: env_path("MEGATRON_BOT_STUDIO_TOKEN_FILE", home().join(".config/homebase/token")),
            comfy_url: env_string("MEGATRON_BOT_COMFY_URL", "http://127.0.0.1:8188").trim_end_matches('/').to_string(),
            probe_timeout: PROBE_TIMEOUT,
        }
    }

    fn game(&self) -> Option<String> {
        if let Some(reason) = self.cgroup_game_scope() { return Some(reason); }
        self.proc_game_process()
    }

    /// A live Steam game scope anywhere in the user's cgroup tree (the Steam client itself uses another name).
    fn cgroup_game_scope(&self) -> Option<String> {
        let mut roots = Vec::new();
        let user_slice = self.cgroup_root.join("user.slice");
        if let Ok(entries) = std::fs::read_dir(&user_slice) {
            for e in entries.flatten() {
                if !e.path().is_dir() { continue; }
                let name = e.file_name().to_string_lossy().to_string();
                if !name.starts_with("user-") { continue; }
                if let Ok(services) = std::fs::read_dir(e.path()) {
                    for s in services.flatten() {
                        if s.path().is_dir() && s.file_name().to_string_lossy().starts_with("user@") {
                            roots.push(s.path());
                        }
                    }
                }
            }
        }
        if roots.is_empty() { roots.push(self.cgroup_root.clone()); }
        let system_steam = self.cgroup_root.join("system.slice/steam.slice");
        if system_steam.exists() { roots.push(system_steam); }

        let mut stack: Vec<(PathBuf, usize)> = roots.into_iter().map(|r| (r, 0)).collect();
        let mut seen = std::collections::HashSet::new();
        let mut visited = 0;
        while let Some((dir, depth)) = stack.pop() {
            if depth > 8 || visited > 2000 { continue; }
            visited += 1;
            if !seen.insert(dir.clone()) { continue; }
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                let path = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with("app-steam-app") && name.ends_with(".scope") {
                    if let Some(pid) = first_pid(&path.join("cgroup.procs")) { return Some(format!("process {pid} in {name}")); }
                } else if path.is_dir() {
                    stack.push((path, depth + 1));
                }
            }
        }
        None
    }

    /// Walk /proc: a process whose name is one of the owner's game names, or whose executable or command line lives
    /// under a Steam library's steamapps/common/ (Proton games such as deadlock.exe).
    fn proc_game_process(&self) -> Option<String> {
        let entries = std::fs::read_dir(&self.proc_root).ok()?;
        for e in entries.flatten() {
            let pid = e.file_name().to_string_lossy().to_string();
            if !pid.chars().all(|c| c.is_ascii_digit()) { continue; }
            let dir = e.path();
            if let Ok(comm) = std::fs::read_to_string(dir.join("comm")) {
                let comm = comm.trim();
                if self.game_processes.iter().any(|g| g == comm) { return Some(format!("process {comm}")); }
            }
            if let Ok(exe) = std::fs::read_link(dir.join("exe")) {
                let exe = exe.to_string_lossy().to_string();
                if let Some(base) = exe.rsplit('/').next() {
                    if self.game_processes.iter().any(|g| g == base) { return Some(format!("process {base}")); }
                }
                if is_steam_common(&exe) { return Some(format!("a game under steamapps/common ({exe})")); }
            }
            if let Ok(raw) = std::fs::read(dir.join("cmdline")) {
                for arg in raw.split(|b| *b == 0) {
                    let arg = String::from_utf8_lossy(arg);
                    if is_steam_common(&arg) && names_a_program(&arg) { return Some(format!("a game under steamapps/common ({arg})")); }
                }
            }
        }
        None
    }

    fn studio_busy(&self) -> bool {
        if self.lock_fresh() { return true; }
        if self.studio_video_rendering() { return true; }
        self.comfy_running()
    }

    fn lock_fresh(&self) -> bool {
        std::fs::metadata(&self.studio_lock)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age < STUDIO_LOCK_MAX_AGE)
    }

    /// Studio's own view, when its shared token is readable. A 401 or an unreachable server simply means "not this".
    fn studio_video_rendering(&self) -> bool {
        let agent = ureq::AgentBuilder::new().timeout(self.probe_timeout).build();
        let mut req = agent.get(&format!("{}/api/state", self.studio_url));
        if let Ok(token) = std::fs::read_to_string(&self.token_file) {
            let token = token.trim();
            if !token.is_empty() { req = req.set("X-Homebase-Token", token); }
        }
        match req.call() {
            Ok(r) => r.into_json::<Value>().ok()
                .and_then(|v| v.get("video_render").and_then(Value::as_bool))
                .unwrap_or(false),
            Err(_) => false,
        }
    }

    /// ComfyUI is only running a queue item when it is actually on the card.
    fn comfy_running(&self) -> bool {
        let agent = ureq::AgentBuilder::new().timeout(self.probe_timeout).build();
        match agent.get(&format!("{}/queue", self.comfy_url)).call() {
            Ok(r) => r.into_json::<Value>().ok()
                .and_then(|v| v.get("queue_running").and_then(Value::as_array).map(|a| !a.is_empty()))
                .unwrap_or(false),
            Err(_) => false,
        }
    }
}

impl GpuCheck for GpuGate {
    fn blocked(&self) -> Option<Pause> {
        if let Some(reason) = self.game() { return Some(Pause::Game(reason)); }
        if self.studio_busy() { return Some(Pause::Studio); }
        None
    }
}

fn first_pid(procs: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(procs).ok()?;
    text.lines().next()?.trim().parse().ok()
}

/// Is this a path under a Steam library's steamapps/common/? Both separators, since Proton games carry Windows paths.
pub fn is_steam_common(s: &str) -> bool {
    let low = s.to_ascii_lowercase();
    low.contains("/steamapps/common/") || low.contains("\\steamapps\\common\\")
}

/// Does this command-line argument name a program to run, rather than merely mention a path? Only then can it mark a
/// game. A shell command, a script or a tool that happens to pass a Steam folder (a symlink to an install, a test run
/// reading a game's data files) is not a game: that is how a terminal command once kept Oracle "paused for a game".
/// A program here: an absolute path (Unix, or a Windows drive path from Proton), no shell syntax, and either a Windows
/// .exe or an existing regular file someone may execute.
pub fn names_a_program(arg: &str) -> bool {
    let a = arg.trim();
    if a.is_empty() || a.chars().any(|c| matches!(c, ';' | '&' | '|' | '$' | '`' | '"' | '\'' | '\n' | '<' | '>')) { return false; }
    let b = a.as_bytes();
    let windows = b.len() > 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/');
    if windows { return a.to_ascii_lowercase().ends_with(".exe"); }
    if !a.starts_with('/') { return false; }
    if a.to_ascii_lowercase().ends_with(".exe") { return true; }
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(a).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

/// The owner's game names from the voice app's settings (read only), else the same defaults Studio uses.
pub fn read_game_processes(path: &Path) -> Vec<String> {
    if let Ok(text) = std::fs::read_to_string(path) {
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            if let Some(names) = v.get("game_processes").and_then(Value::as_array) {
                let names: Vec<String> = names.iter().filter_map(Value::as_str)
                    .map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                if !names.is_empty() { return names; }
            }
        }
    }
    DEFAULT_GAME_PROCESSES.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_program_under_steamapps_common_is_a_game() {
        // a terminal command that mentions a Steam folder is not a game
        assert!(!names_a_program("source ~/.snap.sh && ln -s \"/home/u/.local/share/Steam/steamapps/common/Distant Worlds Universe\" x"));
        assert!(!names_a_program("cd /home/u/.local/share/Steam/steamapps/common/Game; ls"));
        // a folder is not a program
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("steamapps/common/Game");
        std::fs::create_dir_all(&game).unwrap();
        assert!(!names_a_program(game.to_str().unwrap()));
        // a data file is not a program; an executable file is
        let data = game.join("races.txt");
        std::fs::write(&data, "x").unwrap();
        assert!(!names_a_program(data.to_str().unwrap()));
        let bin = game.join("game.x86_64");
        std::fs::write(&bin, "x").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(names_a_program(bin.to_str().unwrap()));
        // Proton games carry Windows paths
        assert!(names_a_program("Z:\\home\\u\\.local\\share\\Steam\\steamapps\\common\\Deadlock\\game\\bin\\win64\\deadlock.exe"));
        assert!(!names_a_program("Z:\\home\\u\\.local\\share\\Steam\\steamapps\\common\\Deadlock\\game\\readme.txt"));
    }

    #[test]
    fn steam_common_is_recognised_with_either_separator() {
        assert!(is_steam_common("/home/x/.local/share/Steam/steamapps/common/Deadlock/Deadlock.exe"));
        assert!(is_steam_common(r"Z:\SteamLibrary\steamapps\common\Deadlock\deadlock.exe"));
        assert!(!is_steam_common("/home/x/.local/share/Steam/steamapps/workshop"));
        assert!(!is_steam_common("/usr/bin/dwu"));
    }

    #[test]
    fn game_names_come_from_voice_settings_or_the_studio_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"game_processes": ["deadlock", " dwu "]}"#).unwrap();
        assert_eq!(read_game_processes(&path), vec!["deadlock".to_string(), "dwu".to_string()]);
        std::fs::write(&path, r#"{"game_processes": []}"#).unwrap();
        assert_eq!(read_game_processes(&path), DEFAULT_GAME_PROCESSES.to_vec());
        assert_eq!(read_game_processes(&dir.path().join("missing.json")), DEFAULT_GAME_PROCESSES.to_vec());
    }

    #[test]
    fn pause_replies_are_the_owners_words() {
        assert_eq!(Pause::Game("process dwu".into()).reply(), "I'm paused while a game is running.");
        assert_eq!(Pause::Studio.reply(), "I'm paused while Studio is making something.");
    }
}
