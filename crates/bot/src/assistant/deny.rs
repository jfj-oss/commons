//! What the assistant may never touch, in any tier, by any model: the same list feeds the local file tools, the
//! pre-check on every request Claude Code makes, and the Claude Code settings file.
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};

/// Paths (relative to the home folder) that are never read or written. Directories cover everything below them.
const DENY_REL: &[&str] = &[
    ".ssh", ".gnupg", ".config/deepseek-key", ".config/homebase/token", "Downloads/github-recovery-codes.txt",
    ".local/share/commons-host", ".local/share/megatron-bot", ".local/share/megatron-brave", ".local/share/liner/liner.db",
    // password and key stores
    ".local/share/keyrings", ".local/share/kwalletd", ".kde4/share/apps/kwallet", ".kde/share/apps/kwallet", ".password-store",
    ".config/Bitwarden", ".config/Bitwarden CLI", ".config/KeePassXC", ".local/share/KeePassXC", ".config/1Password", ".pki", ".netrc",
    ".git-credentials", ".aws", ".config/gh", ".docker/config.json", ".config/rclone", ".kube",
    // browser profiles
    ".config/BraveSoftware", ".config/google-chrome", ".config/chromium", ".config/chromium-hardened", ".config/microsoft-edge",
    ".config/vivaldi", ".config/opera", ".mozilla", ".librewolf", ".zen", ".var/app/org.mozilla.firefox", ".var/app/com.brave.Browser",
    ".var/app/org.chromium.Chromium", ".cache/BraveSoftware", ".cache/chromium", ".cache/google-chrome", ".cache/mozilla",
    // Claude's own login and settings
    ".claude/.credentials.json", ".claude.json",
    // mega's own account folders (desktop app, browser edition and its window profile, the bridge secret in its install folder), config, and other cloud logins
    ".local/share/app.commons.chat", ".local/share/app.commons.chat.browser-profile", ".config/app.commons.chat", ".config/commons",
    ".local/share/commons", ".local/share/megatron-browser", ".local/share/megatron", ".cache/app.commons.chat",
    ".config/gcloud", ".azure", ".config/fish", ".local/share/fish",
    // shell histories (often hold pasted secrets)
    ".bash_history", ".zsh_history", ".zhistory", ".sh_history", ".python_history", ".psql_history", ".mysql_history", ".node_repl_history",
];

/// Extra words that, appearing anywhere in a command or path text, mark it as touching a protected place. Catches `cd ~ && cat .ssh/id_rsa`.
const DENY_TOKENS: &[&str] = &[
    ".ssh", ".gnupg", "deepseek-key", "homebase/token", "github-recovery-codes", "share/commons-host", "share/megatron-bot", "share/megatron-brave", "liner.db",
    "keyrings", "kwallet", ".password-store", "Bitwarden", "KeePassXC", "BraveSoftware", ".mozilla", ".librewolf", "google-chrome",
    "config/chromium", "app.commons.chat", "share/commons-", "share/megatron-browser", "bridge-secret", "_history", "fish_history", "config/gcloud", ".azure", "config/fish", ".netrc", ".git-credentials", ".credentials.json", "/etc/shadow", "/etc/sudoers", "/etc/gshadow",
];

/// Places Claude may read but must not change, because changing them hands it more power than the owner approved
/// (its own settings, the bot and Claude binaries, shell start-up files, systemd user units, autostart).
const NO_WRITE_REL: &[&str] = &[
    ".claude", ".local/bin", ".bashrc", ".bash_profile", ".bash_login", ".profile", ".zshrc", ".zprofile", ".zshenv", ".config/fish",
    ".config/systemd", ".config/autostart", ".config/environment.d", ".local/share/applications", ".config/git", ".gitconfig",
];

/// File names the *local model's* file tools refuse everywhere (injection bait): secrets inside project folders.
const SECRET_NAMES: &[&str] = &[".env", ".netrc", ".npmrc", ".pypirc", "id_rsa", "id_ed25519", "id_ecdsa", "id_dsa", "credentials", "secrets", "authorized_keys"];
const SECRET_SUFFIX: &[&str] = &[".pem", ".key", ".p12", ".pfx", ".kdbx", ".gpg", ".asc", ".keystore"];
const SECRET_WORDS: &[&str] = &["password", "passwd", "secret", "token", "apikey", "api_key", "api-key", "recovery-code", "private"];

/// Commands that give root or another identity. Denied for Claude Code, everywhere.
const SUDO_WORDS: &[&str] = &["sudo", "doas", "pkexec", "su", "sudoedit", "run0"];

/// Network tools other than Claude's own web search and fetch.
const NET_TOOLS: &[&str] = &["curl", "wget", "nc", "ncat", "netcat", "socat", "ssh", "scp", "sftp", "rsync", "telnet", "ftp", "tftp", "nmap", "aria2c", "lftp"];

#[derive(Debug, Clone)]
pub struct Deny {
    pub home: PathBuf,
    /// Absolute paths, each with its symlink-resolved twin when that differs.
    paths: Vec<PathBuf>,
    no_write: Vec<PathBuf>,
}

/// `/a/./b/../c` becomes `/a/c`, without touching the disk.
pub fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => { out.pop(); }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The real path of something that may not exist yet: the longest existing ancestor is resolved through symlinks.
pub fn resolve(p: &Path) -> PathBuf {
    let p = lexical(p);
    let mut tail: Vec<std::ffi::OsString> = vec![];
    let mut cur = p.clone();
    loop {
        if let Ok(real) = cur.canonicalize() {
            let mut out = real;
            for t in tail.iter().rev() { out.push(t); }
            return out;
        }
        match (cur.file_name().map(|n| n.to_os_string()), cur.parent().map(Path::to_path_buf)) {
            (Some(n), Some(par)) => { tail.push(n); cur = par; }
            _ => return p,
        }
    }
}

impl Deny {
    pub fn new(home: &Path, extra: &[PathBuf]) -> Deny {
        let mut paths: Vec<PathBuf> = vec![];
        let add = |v: &mut Vec<PathBuf>, p: PathBuf| {
            let real = resolve(&p);
            if !v.contains(&p) { v.push(p.clone()); }
            if real != p && !v.contains(&real) { v.push(real); }
        };
        for rel in DENY_REL { add(&mut paths, home.join(rel)); }
        // every ~/.local/share/commons-* folder (the host, trial copies, whatever else a build leaves there)
        if let Ok(rd) = std::fs::read_dir(home.join(".local/share")) {
            for e in rd.flatten() { if e.file_name().to_string_lossy().starts_with("commons-") { add(&mut paths, e.path()); } }
        }
        for p in extra { add(&mut paths, lexical(p)); }
        add(&mut paths, PathBuf::from("/etc/shadow"));
        add(&mut paths, PathBuf::from("/etc/gshadow"));
        add(&mut paths, PathBuf::from("/etc/sudoers"));
        add(&mut paths, PathBuf::from("/etc/sudoers.d"));
        let mut no_write = vec![];
        for rel in NO_WRITE_REL { add(&mut no_write, home.join(rel)); }
        Deny { home: home.to_path_buf(), paths, no_write }
    }

    pub fn paths(&self) -> &[PathBuf] { &self.paths }

    fn under(p: &Path, base: &Path) -> bool { p == base || p.starts_with(base) }

    /// Whether this path (as written, and as it resolves through symlinks) is inside a protected place.
    pub fn is_denied(&self, p: &Path) -> bool {
        let lex = lexical(p);
        let real = resolve(&lex);
        self.paths.iter().any(|d| Self::under(&lex, d) || Self::under(&real, d))
    }

    /// Whether Claude may not write here either (protected places plus the no-write list).
    pub fn is_write_denied(&self, p: &Path) -> bool {
        if self.is_denied(p) { return true; }
        let lex = lexical(p);
        let real = resolve(&lex);
        self.no_write.iter().any(|d| Self::under(&lex, d) || Self::under(&real, d))
    }

    /// For the local model's file tools only: is this a file that looks like a secret, wherever it lives?
    pub fn looks_secret(p: &Path) -> bool {
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else { return false };
        let n = name.to_ascii_lowercase();
        n == ".env" || n.starts_with(".env.") || SECRET_NAMES.contains(&n.as_str()) || SECRET_SUFFIX.iter().any(|s| n.ends_with(s))
            || SECRET_WORDS.iter().any(|w| n.contains(w))
    }

    /// Does this free text (a shell command, a path, a diff) mention a protected place in any spelling we can recognise?
    pub fn mentions_protected(&self, text: &str) -> Option<String> {
        let home = self.home.to_string_lossy().to_string();
        let flat = text.replace("${HOME}", "~").replace("$HOME", "~").replace(&home, "~");
        for t in DENY_TOKENS { if flat.contains(t) { return Some((*t).to_string()); } }
        for d in &self.paths {
            let s = d.to_string_lossy();
            if text.contains(s.as_ref()) { return Some(s.to_string()); }
        }
        None
    }
}

/// Words of a command line, split on anything that cannot be part of a program name.
fn words(cmd: &str) -> Vec<&str> {
    cmd.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-'))).filter(|w| !w.is_empty()).collect()
}

/// Any use of sudo (or doas, pkexec, su, ...) as a word of the command.
pub fn mentions_sudo(cmd: &str) -> bool {
    words(cmd).iter().any(|w| {
        let base = w.rsplit('/').next().unwrap_or(w);
        SUDO_WORDS.contains(&base)
    })
}

/// The Claude Code settings file: deny rules (they beat every allow rule and every approval), no allow rules, and the
/// sandbox on for shell commands (fails closed when it cannot start).
pub fn claude_settings(d: &Deny, allowed_domains: &[String]) -> Value {
    let abs = |p: &Path| format!("/{}", p.to_string_lossy());
    let mut deny: Vec<String> = vec![];
    let mut deny_read_paths: Vec<String> = vec![];
    for p in d.paths() {
        let a = abs(p);
        for tool in ["Read", "Edit"] {
            deny.push(format!("{tool}({a})"));
            deny.push(format!("{tool}({a}/**)"));
        }
        deny_read_paths.push(p.to_string_lossy().to_string());
    }
    // Secret-looking files anywhere (a project's .env, keys, keystores), for Claude too, even inside the read dirs it may read freely.
    for pat in ["**/.env", "**/.env.*", "**/*.pem", "**/*.key", "**/*.p12", "**/*.pfx", "**/*.kdbx", "**/*.keystore", "**/id_rsa*", "**/id_ed25519*", "**/id_ecdsa*", "**/.netrc", "**/.npmrc", "**/.pypirc", "**/credentials*", "**/*secret*", "**/*token*"] {
        deny.push(format!("Read({pat})"));
        deny.push(format!("Edit({pat})"));
    }
    let mut deny_write_paths = deny_read_paths.clone();
    for p in &d.no_write {
        let a = abs(p);
        deny.push(format!("Edit({a})"));
        deny.push(format!("Edit({a}/**)"));
        deny_write_paths.push(p.to_string_lossy().to_string());
    }
    for t in DENY_TOKENS { deny.push(format!("Bash(*{t}*)")); }
    for w in SUDO_WORDS {
        deny.push(format!("Bash({w})"));
        deny.push(format!("Bash({w} *)"));
        deny.push(format!("Bash(*; {w} *)"));
        deny.push(format!("Bash(*&& {w} *)"));
        deny.push(format!("Bash(*| {w} *)"));
        deny.push(format!("Bash(*/{w} *)"));
    }
    deny.push("Bash(sudo:*)".into());
    for t in NET_TOOLS { deny.push(format!("Bash({t} *)")); deny.push(format!("Bash(*&& {t} *)")); deny.push(format!("Bash(*; {t} *)")); deny.push(format!("Bash(*| {t} *)")); }
    deny.sort();
    deny.dedup();
    let mut sandbox = json!({
        "enabled": true,
        "failIfUnavailable": true,
        "autoAllowBashIfSandboxed": false,
        "allowUnsandboxedCommands": false,
        "filesystem": { "denyRead": deny_read_paths, "denyWrite": deny_write_paths },
    });
    if !allowed_domains.is_empty() { sandbox["network"] = json!({ "allowedDomains": allowed_domains }); }
    json!({
        // "ask" on every shell command, so even the ones Claude Code treats as read-only (ls, cat, df...) come to the room
        "permissions": { "deny": deny, "ask": ["Bash", "WebFetch", "WebSearch"], "allow": [], "defaultMode": "default" },
        "sandbox": sandbox,
        "includeCoAuthoredBy": false,
        "enableAllProjectMcpServers": false,
    })
}

/// Reasons not to start Claude at all: its user settings file would grant permissions or run hooks that the assistant
/// room's approvals could not see.
pub fn user_settings_problem(config_dir: &Path) -> Option<String> {
    let text = match std::fs::read_to_string(config_dir.join("settings.json")) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => return Some("it cannot be read, so I cannot tell what it allows".into()),
    };
    // Claude Code would read a file this program cannot parse in its own, more lenient way, so unreadable means unsafe
    let Ok(v) = serde_json::from_str::<Value>(&text) else { return Some("it cannot be parsed, so I cannot tell what it allows".into()) };
    for key in ["apiKeyHelper", "statusLine", "awsAuthRefresh", "awsCredentialExport", "otelHeadersHelper", "enabledPlugins", "extraKnownMarketplaces", "plugins", "pluginConfigs"] {
        if !v[key].is_null() { return Some(format!("it sets {key}, which can run programs without approval")); }
    }
    if v["permissions"]["allow"].as_array().is_some_and(|a| !a.is_empty()) { return Some("it has allow rules (permissions.allow), which would skip approvals".into()); }
    if v["permissions"]["defaultMode"].as_str().is_some_and(|m| m != "default") { return Some("its default permission mode is not 'default'".into()); }
    if !v["hooks"].is_null() && !v["hooks"].as_object().is_some_and(|h| h.is_empty()) { return Some("it defines hooks, which would run commands without approval".into()); }
    if v["skipDangerousModePermissionPrompt"].as_bool() == Some(true) || v["env"].as_object().is_some_and(|e| !e.is_empty()) { return Some("it sets environment variables or skips permission prompts".into()); }
    None
}
