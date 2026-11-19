//! Settings for the assistant room, all from the environment. Nothing is on unless an owner account is named, and
//! `MEGATRON_BOT_ASSIST=0` switches the whole feature off at startup.
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct AssistConfig {
    /// The one account (hex id) that may use an assistant room.
    pub owner: String,
    pub home: PathBuf,
    /// Where the bot keeps its files (state, log, per-task sockets, the Claude settings file).
    pub data_dir: PathBuf,
    /// Folders the local model may list and read, nothing outside them.
    pub roots: Vec<PathBuf>,
    /// User units the local model may restart.
    pub units: Vec<String>,
    pub claude_bin: PathBuf,
    /// The program Claude Code starts as its permission-prompt server (this very binary, subcommand `assist-mcp`).
    pub self_exe: PathBuf,
    pub use_systemd_run: bool,
    pub rune_url: String,
    pub approval_timeout: Duration,
    pub task_timeout: Duration,
    /// Extra environment for the Claude process (tests only; the process otherwise gets a minimal, cleaned environment).
    pub extra_env: Vec<(String, String)>,
    /// Sites a command run by Claude may reach (the sandbox proxy). Empty: none.
    pub allowed_domains: Vec<String>,
    /// Claude Code's own config folder, whose settings.json is checked before every task.
    pub claude_config_dir: PathBuf,
    /// Where Claude Code runs. Not the home folder, so reads and edits elsewhere go through the approver.
    pub workspace: PathBuf,
    /// Extra folders Claude may read without asking (the deny list still applies).
    pub read_dirs: Vec<PathBuf>,
    pub max_read_bytes: usize,
    pub max_list_entries: usize,
    /// One posted message is at most this many characters; longer text goes out as several.
    pub chunk_chars: usize,
    /// Progress messages per task (the approvals and the final answer are not counted).
    pub max_progress: usize,
    pub progress_gap: Duration,
    /// Hand the session bus and display variables (D-Bus, X11, Wayland, XDG_RUNTIME_DIR) to Claude Code, so `systemctl --user` works.
    pub desktop_env: bool,
    /// Keep the protected places (keys, tokens, password stores, browser profiles, the bot's own data) out of reach. Default on;
    /// `MEGATRON_BOT_ASSIST_PROTECT_SECRETS=0` turns the whole protection off.
    pub protect_secrets: bool,
    /// The snapper config whose snapshot is taken before each Claude task (`None`: no snapshots).
    pub snapshot_config: Option<String>,
    /// How many task snapshots to keep.
    pub snapshot_keep: usize,
}

pub fn valid_account(s: &str) -> bool { s.len() >= 6 && s.chars().all(|c| c.is_ascii_hexdigit()) }

fn env_string(name: &str) -> Option<String> { std::env::var(name).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) }

pub fn expand_home(s: &str, home: &Path) -> PathBuf {
    if s == "~" { home.to_path_buf() }
    else if let Some(rest) = s.strip_prefix("~/") { home.join(rest) }
    else { PathBuf::from(s) }
}

impl AssistConfig {
    pub fn new(owner: &str, home: &Path, data_dir: &Path) -> AssistConfig {
        AssistConfig {
            owner: owner.to_ascii_lowercase(),
            home: home.to_path_buf(),
            data_dir: data_dir.to_path_buf(),
            roots: ["projects", "Documents", "Downloads"].iter().map(|d| home.join(d)).collect(),
            units: vec![],
            claude_bin: home.join(".local/bin/claude"),
            self_exe: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("megatron-bot")),
            use_systemd_run: true,
            rune_url: "http://127.0.0.1:4545".into(),
            approval_timeout: Duration::from_secs(300),
            task_timeout: Duration::from_secs(3600),
            extra_env: vec![],
            allowed_domains: vec![],
            claude_config_dir: std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home.join(".claude")),
            workspace: home.join(".local/share/oracle-workspace"),
            read_dirs: vec![home.join("projects")],
            max_read_bytes: 24_000,
            max_list_entries: 200,
            chunk_chars: 1800,
            max_progress: 5,
            progress_gap: Duration::from_secs(25),
            desktop_env: true,
            protect_secrets: true,
            snapshot_config: Some("home".into()),
            snapshot_keep: 10,
        }
    }

    /// `None` when the feature is off: switched off with `MEGATRON_BOT_ASSIST=0`, or no (valid) owner account configured.
    pub fn from_env(data_dir: &Path) -> Option<AssistConfig> {
        if env_string("MEGATRON_BOT_ASSIST").is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "0" | "off" | "false" | "no")) {
            eprintln!("[bot] assistant room: switched off (MEGATRON_BOT_ASSIST=0)");
            return None;
        }
        let owner = env_string("MEGATRON_BOT_OWNER_ACCOUNT")?;
        if !valid_account(&owner) {
            eprintln!("[bot] assistant room: MEGATRON_BOT_OWNER_ACCOUNT is not a hex account id; the feature stays off");
            return None;
        }
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        let mut cfg = AssistConfig::new(&owner, &home, data_dir);
        let roots = env_string("MEGATRON_BOT_ASSIST_ROOTS").unwrap_or_else(|| "~/projects:~/Documents:~/Downloads".into());
        cfg.roots = roots.split(':').map(str::trim).filter(|s| !s.is_empty()).map(|s| expand_home(s, &home)).collect();
        cfg.units = env_string("MEGATRON_BOT_ASSIST_UNITS").map(|s| s.split([',', ' ', ':']).filter(|u| !u.is_empty()).map(String::from).collect()).unwrap_or_default();
        if let Some(d) = env_string("MEGATRON_BOT_ASSIST_READ_DIRS") { cfg.read_dirs = d.split(':').map(str::trim).filter(|s| !s.is_empty()).map(|s| expand_home(s, &home)).collect(); }
        if let Some(w) = env_string("MEGATRON_BOT_ASSIST_WORKSPACE") { cfg.workspace = expand_home(&w, &home); }
        if let Some(b) = env_string("MEGATRON_BOT_ASSIST_CLAUDE") { cfg.claude_bin = expand_home(&b, &home); }
        if let Some(u) = env_string("MEGATRON_BOT_RUNE_URL") { cfg.rune_url = u; }
        if let Some(d) = env_string("MEGATRON_BOT_ASSIST_DOMAINS") { cfg.allowed_domains = d.split([',', ' ']).filter(|x| !x.is_empty()).map(String::from).collect(); }
        let off = |name: &str| env_string(name).is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "0" | "off" | "false" | "no"));
        if off("MEGATRON_BOT_ASSIST_DESKTOP_ENV") { cfg.desktop_env = false; }
        if off("MEGATRON_BOT_ASSIST_PROTECT_SECRETS") { cfg.protect_secrets = false; eprintln!("[bot] assistant room: protection of secret places is OFF (MEGATRON_BOT_ASSIST_PROTECT_SECRETS=0)"); }
        if off("MEGATRON_BOT_ASSIST_SNAPSHOT") { cfg.snapshot_config = None; } else if let Some(c) = env_string("MEGATRON_BOT_ASSIST_SNAPSHOT_CONFIG") { cfg.snapshot_config = Some(c); }
        Some(cfg)
    }
}
