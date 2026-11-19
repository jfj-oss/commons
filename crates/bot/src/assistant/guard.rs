//! The destruction guard: every tool call Claude Code makes in the assistant room passes through `check` before it is allowed.
//! Nothing is asked of the owner; a call that would delete or overwrite something is refused with a short reason the model sees.
//!
//! The shell command is parsed (quotes, `$(...)`, backticks, pipes, `;`/`&&`/`||`, heredocs, `bash -c`, `xargs`, `find -exec`,
//! `env`/`nohup`/`timeout` wrappers, script files it is about to run) and each simple command is judged on its own. When a command
//! cannot be understood (the program name is built from a variable, a shell reads commands from a pipe, a redirect target is
//! a variable) it is refused rather than guessed at. Editing files (Edit, appending, `sed -i`, new files) is fine; removing,
//! truncating, and replacing an existing file wholesale is not.
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Where relative paths point and what `~` means while one command line is read.
pub struct Ctx { pub home: PathBuf, pub cwd: Option<PathBuf> }

/// `None`: allowed. `Some(reason)`: refused, with a one-sentence reason.
pub fn check(name: &str, input: &Value, home: &Path, cwd: &Path) -> Option<String> {
    let mut ctx = Ctx { home: home.to_path_buf(), cwd: Some(cwd.to_path_buf()) };
    match name {
        "Bash" | "BashOutput" if input["command"].is_string() => check_script(input["command"].as_str().unwrap_or(""), &mut ctx, 0).err(),
        "Write" => {
            let p = input["file_path"].as_str().unwrap_or("");
            let path = ctx.resolve(p)?;
            if std::fs::symlink_metadata(&path).is_ok() { Some(format!("Write would replace the existing file {}. Change it with Edit, or write to a new file name.", path.display())) } else { None }
        }
        "NotebookEdit" if input["edit_mode"].as_str() == Some("delete") => Some("deleting a notebook cell is not allowed".into()),
        _ => None,
    }
}

impl Ctx {
    fn resolve(&self, raw: &str) -> Option<PathBuf> {
        let p = super::config::expand_home(raw, &self.home);
        if p.is_absolute() { Some(super::deny::lexical(&p)) } else { self.cwd.as_ref().map(|c| super::deny::lexical(&c.join(p))) }
    }
}

// --------------------------------------------------------------------- reading a shell command line

#[derive(Clone, Debug)]
struct Word { t: String, dynamic: bool }
#[derive(Debug, Default)]
struct Cmd { words: Vec<Word>, redirs: Vec<(String, Option<Word>)>, piped_in: bool }
#[derive(Default)]
struct Parsed { cmds: Vec<Cmd>, subs: Vec<String>, heredocs: Vec<String> }

struct P<'a> {
    c: Vec<char>, i: usize, home: &'a str, out: Parsed, cur: Cmd,
    w: String, in_w: bool, dynw: bool, quoted: bool,
    pending: Option<String>, hd: Vec<(String, bool)>,
}

impl P<'_> {
    fn end_word(&mut self) {
        if !self.in_w { return; }
        let word = Word { t: std::mem::take(&mut self.w), dynamic: self.dynw };
        self.in_w = false; self.dynw = false; self.quoted = false;
        match self.pending.take() { Some(op) => self.cur.redirs.push((op, Some(word))), None => self.cur.words.push(word) }
    }
    fn end_cmd(&mut self, piped: bool) {
        self.end_word();
        if let Some(op) = self.pending.take() { self.cur.redirs.push((op, None)); }
        let c = std::mem::replace(&mut self.cur, Cmd { piped_in: piped, ..Default::default() });
        if !c.words.is_empty() || !c.redirs.is_empty() { self.out.cmds.push(c); }
    }
    fn peek(&self, k: usize) -> Option<char> { self.c.get(self.i + k).copied() }
    fn push(&mut self, ch: char) { self.w.push(ch); self.in_w = true; }
    fn dynamic(&mut self) { self.w.push('\u{1}'); self.dynw = true; self.in_w = true; }

    /// `self.i` is just after the opening bracket; returns the inside and moves past the closing one.
    fn inner_parens(&mut self) -> String {
        let (mut depth, mut s) = (1usize, String::new());
        while let Some(ch) = self.peek(0) {
            self.i += 1;
            match ch {
                '\\' => { s.push(ch); if let Some(n) = self.peek(0) { s.push(n); self.i += 1; } continue; }
                '\'' => { s.push(ch); while let Some(n) = self.peek(0) { self.i += 1; s.push(n); if n == '\'' { break; } } continue; }
                '"' => { s.push(ch); while let Some(n) = self.peek(0) { self.i += 1; s.push(n); if n == '\\' { if let Some(m) = self.peek(0) { s.push(m); self.i += 1; } } else if n == '"' { break; } } continue; }
                '(' => depth += 1,
                ')' => { depth -= 1; if depth == 0 { return s; } }
                _ => {}
            }
            s.push(ch);
        }
        s
    }

    fn dollar(&mut self) {
        // self.c[self.i] == '$'
        match self.peek(1) {
            Some('(') => { self.i += 2; let inner = self.inner_parens(); self.out.subs.push(inner); self.dynamic(); }
            Some('{') => {
                self.i += 2;
                let mut name = String::new();
                while let Some(ch) = self.peek(0) { self.i += 1; if ch == '}' { break; } name.push(ch); }
                if name == "HOME" { let h = self.home.to_string(); self.w.push_str(&h); self.in_w = true; } else { self.dynamic(); }
            }
            Some(n) if n.is_ascii_alphanumeric() || n == '_' || "@*#?$!-".contains(n) => {
                self.i += 1;
                let mut name = String::new();
                if n.is_ascii_alphabetic() || n == '_' {
                    while let Some(ch) = self.peek(0) { if ch.is_ascii_alphanumeric() || ch == '_' { name.push(ch); self.i += 1; } else { break; } }
                } else { name.push(n); self.i += 1; }
                if name == "HOME" { let h = self.home.to_string(); self.w.push_str(&h); self.in_w = true; } else { self.dynamic(); }
            }
            _ => { self.i += 1; self.push('$'); }
        }
    }

    fn backtick(&mut self) {
        self.i += 1;
        let mut s = String::new();
        while let Some(ch) = self.peek(0) {
            self.i += 1;
            if ch == '`' { break; }
            if ch == '\\' { if let Some(n) = self.peek(0) { self.i += 1; if n != '`' && n != '\\' && n != '$' { s.push('\\'); } s.push(n); continue; } }
            s.push(ch);
        }
        self.out.subs.push(s);
        self.dynamic();
    }

    fn heredoc_delim(&mut self) {
        while matches!(self.peek(0), Some(' ' | '\t')) { self.i += 1; }
        let mut d = String::new();
        while let Some(ch) = self.peek(0) {
            if ch.is_whitespace() || matches!(ch, ';' | '&' | '|' | '(' | ')' | '<' | '>') { break; }
            self.i += 1;
            match ch {
                '\'' | '"' => { while let Some(n) = self.peek(0) { self.i += 1; if n == ch { break; } d.push(n); } }
                '\\' => {}
                _ => d.push(ch),
            }
        }
        if !d.is_empty() { self.hd.push((d, false)); }
    }

    fn read_heredoc_bodies(&mut self) {
        let todo = std::mem::take(&mut self.hd);
        for (delim, _) in todo {
            let mut body = String::new();
            loop {
                if self.i >= self.c.len() { break; }
                let mut line = String::new();
                while let Some(ch) = self.peek(0) { self.i += 1; if ch == '\n' { break; } line.push(ch); }
                if line.trim_end_matches('\r').trim_start_matches('\t') == delim { break; }
                body.push_str(&line);
                body.push('\n');
            }
            self.out.heredocs.push(body);
        }
    }

    fn run(&mut self) {
        while self.i < self.c.len() {
            let ch = self.c[self.i];
            match ch {
                ' ' | '\t' | '\r' => { self.end_word(); self.i += 1; }
                '\n' => { self.end_cmd(false); self.i += 1; if !self.hd.is_empty() { self.read_heredoc_bodies(); } }
                '#' if !self.in_w => { while self.i < self.c.len() && self.c[self.i] != '\n' { self.i += 1; } }
                '\\' => {
                    match self.peek(1) { Some('\n') => { self.i += 2; } Some(n) => { self.i += 2; self.push(n); self.quoted = true; } None => { self.i += 1; } }
                }
                '\'' => {
                    self.i += 1; self.in_w = true; self.quoted = true;
                    while let Some(n) = self.peek(0) { self.i += 1; if n == '\'' { break; } self.w.push(n); }
                }
                '"' => {
                    self.i += 1; self.in_w = true; self.quoted = true;
                    while let Some(n) = self.peek(0) {
                        match n {
                            '"' => { self.i += 1; break; }
                            '\\' => {
                                match self.peek(1) { Some('\n') => { self.i += 2; } Some(m) if "$\"\\`".contains(m) => { self.i += 2; self.w.push(m); } Some(_) => { self.i += 1; self.w.push('\\'); } None => { self.i += 1; } }
                            }
                            '$' => self.dollar(),
                            '`' => self.backtick(),
                            _ => { self.i += 1; self.w.push(n); }
                        }
                    }
                }
                '$' => self.dollar(),
                '`' => self.backtick(),
                '~' if !self.in_w && matches!(self.peek(1), None | Some('/' | ' ' | '\t' | '\n' | ';' | '&' | '|' | ')')) => { self.i += 1; let h = self.home.to_string(); self.w.push_str(&h); self.in_w = true; }
                '<' | '>' if self.peek(1) == Some('(') => { self.i += 2; let inner = self.inner_parens(); self.out.subs.push(inner); self.dynamic(); }
                '<' | '>' => {
                    if self.in_w && !self.quoted && self.w.chars().all(|c| c.is_ascii_digit()) { self.w.clear(); self.in_w = false; } else { self.end_word(); }
                    let rest: String = self.c[self.i..].iter().take(3).collect();
                    if rest.starts_with("<<<") { self.i += 3; self.pending = Some("<<<".into()); }
                    else if rest.starts_with("<<") { self.i += 2; if self.peek(0) == Some('-') { self.i += 1; } self.heredoc_delim(); }
                    else {
                        let op = ["&>>", ">>", ">|", ">&", "<&", "<>"].iter().find(|o| rest.starts_with(**o)).map(|s| s.to_string()).unwrap_or_else(|| ch.to_string());
                        self.i += op.chars().count();
                        self.pending = Some(op);
                    }
                }
                '&' if self.peek(1) == Some('>') => {
                    self.end_word();
                    let op = if self.peek(2) == Some('>') { self.i += 3; "&>>" } else { self.i += 2; "&>" };
                    self.pending = Some(op.into());
                }
                '&' => { self.i += if self.peek(1) == Some('&') { 2 } else { 1 }; self.end_cmd(false); }
                '|' => {
                    let both = self.peek(1) == Some('|');
                    self.i += if both || self.peek(1) == Some('&') { 2 } else { 1 };
                    self.end_cmd(!both);
                }
                ';' | '(' | ')' => { self.i += 1; self.end_cmd(false); }
                _ => { self.i += 1; self.push(ch); }
            }
        }
        self.end_cmd(false);
    }
}

fn parse(src: &str, home: &str) -> Parsed {
    let mut p = P { c: src.chars().collect(), i: 0, home, out: Parsed::default(), cur: Cmd::default(), w: String::new(), in_w: false, dynw: false, quoted: false, pending: None, hd: vec![] };
    p.run();
    p.out
}

// --------------------------------------------------------------------- judging it

const MAX_DEPTH: u8 = 5;
const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "fish", "ash", "csh", "tcsh", "mksh"];
const INTERPRETERS: &[&str] = &["python", "node", "nodejs", "perl", "ruby", "php", "deno", "bun", "lua", "awk", "gawk", "mawk", "tclsh", "osascript"];
const RESERVED: &[&str] = &["{", "}", "!", "if", "then", "else", "elif", "fi", "do", "done", "while", "until", "case", "esac", "time", "function", "coproc", "select"];
/// Programs that run another command: what follows is judged too, wherever it starts.
const WRAPPERS: &[&str] = &[
    "env", "nohup", "command", "exec", "builtin", "nice", "ionice", "timeout", "setsid", "stdbuf", "flock", "xargs", "watch", "parallel", "busybox",
    "ssh", "chroot", "unshare", "script", "taskset", "systemd-run", "systemd-inhibit", "firejail", "bwrap", "fakeroot", "strace", "ltrace", "chrt", "cpulimit",
    "toybox", "unbuffer", "caffeinate", "setpriv", "runuser", "sg", "nsenter", "at", "batch", "sem", "doas", "sudo", "chronic", "faketime", "proot",
];
/// Programs that only delete or destroy, whatever the arguments.
const DESTROYERS: &[&str] = &[
    "rm", "rmdir", "unlink", "shred", "srm", "wipe", "trash-empty", "trash-rm", "truncate", "mkfs", "wipefs", "fdisk", "sfdisk", "cfdisk", "gdisk", "sgdisk", "cgdisk", "parted",
    "blkdiscard", "mkswap", "lvremove", "vgremove", "pvremove", "lvreduce", "vgreduce", "zerofree", "sdelete", "secure-delete", "git-clean",
];

fn base(w: &str) -> &str { w.rsplit('/').next().unwrap_or(w) }
fn refuse<T>(why: impl Into<String>) -> Result<T, String> { Err(why.into()) }
fn is_assignment(w: &Word) -> bool {
    let Some(eq) = w.t.find('=') else { return false };
    let name = &w.t[..eq];
    !name.is_empty() && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn check_script(src: &str, ctx: &mut Ctx, depth: u8) -> Result<(), String> {
    if depth > MAX_DEPTH { return refuse("commands nested too deeply to check"); }
    let p = parse(src, &ctx.home.to_string_lossy());
    for s in &p.subs { check_script(s, ctx, depth + 1)?; }
    for cmd in &p.cmds { check_cmd(cmd, ctx, depth, &p.heredocs)?; }
    Ok(())
}

fn check_cmd(cmd: &Cmd, ctx: &mut Ctx, depth: u8, heredocs: &[String]) -> Result<(), String> {
    for (op, target) in &cmd.redirs {
        let Some(t) = target else { continue };
        match op.as_str() {
            ">" | ">|" | "&>" => check_overwrite_target(t, ctx, true)?,
            ">&" if !(t.t.chars().all(|c| c.is_ascii_digit()) || t.t == "-") => check_overwrite_target(t, ctx, true)?,
            ">>" | "&>>" | "<>" => check_overwrite_target(t, ctx, false)?,
            _ => {}
        }
    }
    let mut words: &[Word] = &cmd.words;
    loop {
        while words.first().is_some_and(is_assignment) { words = &words[1..]; }
        match words.first() { Some(w) if !w.dynamic && RESERVED.contains(&w.t.as_str()) => words = &words[1..], _ => break }
    }
    if words.is_empty() { return Ok(()); }
    let herestrings: Vec<&Word> = cmd.redirs.iter().filter(|(o, _)| o == "<<<").filter_map(|(_, t)| t.as_ref()).collect();
    check_words(words, cmd.piped_in, ctx, depth, heredocs, &herestrings)
}

/// A redirect (or `tee`) target: a device other than the harmless ones, or (for `>`) a file that already exists.
fn check_overwrite_target(t: &Word, ctx: &Ctx, truncating: bool) -> Result<(), String> {
    if t.dynamic { return refuse("a redirect target built from a variable cannot be checked; use a plain file name"); }
    if let Some(dev) = t.t.strip_prefix("/dev/") {
        if matches!(dev, "null" | "zero" | "stdout" | "stderr" | "stdin" | "tty" | "full" | "random" | "urandom") || dev.starts_with("fd/") { return Ok(()); }
        return refuse(format!("writing to the device {} is not allowed", t.t));
    }
    if t.t.starts_with("/proc/self/fd/") { return Ok(()); }
    if !truncating { return Ok(()); }
    match ctx.resolve(&t.t) {
        None => refuse(format!("cannot tell which folder {} is in; use an absolute path", t.t)),
        Some(p) if std::fs::symlink_metadata(&p).is_ok() => refuse(format!("`>` would overwrite the existing file {}. Append with >>, edit it, or use a new file name", p.display())),
        Some(_) => Ok(()),
    }
}

fn exists(ctx: &Ctx, w: &Word) -> Result<bool, String> {
    if w.dynamic { return refuse(format!("the path {} is built from a variable and cannot be checked", w.t.replace('\u{1}', "$..."))); }
    match ctx.resolve(&w.t) {
        None => refuse(format!("cannot tell which folder {} is in; use an absolute path", w.t)),
        Some(p) => Ok(std::fs::symlink_metadata(&p).is_ok()),
    }
}

fn has_flag(args: &[Word], long: &[&str], short: &[char]) -> bool {
    for a in args {
        if a.t == "--" { break; }
        if let Some(l) = a.t.strip_prefix("--") { let l = l.split('=').next().unwrap_or(""); if long.contains(&l) { return true; } }
        else if a.t.starts_with('-') && a.t.len() > 1 && a.t.chars().skip(1).all(|c| c.is_ascii_alphabetic()) && a.t.chars().skip(1).any(|c| short.contains(&c)) { return true; }
    }
    false
}
fn has_arg(args: &[Word], names: &[&str]) -> bool { args.iter().any(|a| names.contains(&a.t.as_str())) }

fn check_words(words: &[Word], piped_in: bool, ctx: &mut Ctx, depth: u8, heredocs: &[String], herestrings: &[&Word]) -> Result<(), String> {
    let prog = &words[0];
    if prog.dynamic { return refuse("the program to run is built from a variable or substitution, so it cannot be checked"); }
    let b = base(&prog.t);
    let args = &words[1..];

    if DESTROYERS.contains(&b) || b.starts_with("mkfs.") || b.starts_with("mke2fs") { return refuse(format!("{b} deletes or destroys data, which is not allowed")); }
    if b == "dd" {
        for a in args { if let Some(of) = a.t.strip_prefix("of=") { if of != "/dev/null" { return refuse("dd with of= overwrites its target, which is not allowed"); } } }
        return Ok(());
    }
    match b {
        "cd" | "pushd" => {
            ctx.cwd = match args.iter().find(|a| !a.t.starts_with('-')) {
                None => Some(ctx.home.clone()),
                Some(a) if a.dynamic || a.t == "-" => None,
                Some(a) => ctx.resolve(&a.t),
            };
            return Ok(());
        }
        "popd" => { ctx.cwd = None; return Ok(()); }
        "eval" => {
            if args.iter().any(|a| a.dynamic) { return refuse("eval of text built from a variable or substitution cannot be checked"); }
            let text = args.iter().map(|a| a.t.as_str()).collect::<Vec<_>>().join(" ");
            return check_script(&text, ctx, depth + 1);
        }
        "source" | "." => { if let Some(f) = args.first() { return check_script_file(f, ctx, depth); } return Ok(()); }
        "find" | "fd" | "fdfind" => return check_find(args, ctx, depth),
        "git" => return check_git(args),
        "docker" | "podman" | "nerdctl" | "docker-compose" | "podman-compose" | "kubectl" | "helm" => return check_containers(b, args),
        "systemctl" | "loginctl" => {
            if has_arg(args, &["disable", "mask", "revert", "clean"]) { return refuse("disabling, masking or cleaning services is not allowed"); }
            return Ok(());
        }
        "journalctl" => { if args.iter().any(|a| a.t.starts_with("--vacuum")) { return refuse("vacuuming the journal deletes logs, which is not allowed"); } return Ok(()); }
        "snapper" => { if has_arg(args, &["delete", "delete-config", "rollback", "undochange", "cleanup"]) { return refuse("deleting or rolling back snapshots is not allowed"); } return Ok(()); }
        "btrfs" => { if has_arg(args, &["delete", "remove", "del"]) { return refuse("btrfs delete/remove is not allowed"); } return Ok(()); }
        "restic" | "borg" | "kopia" => { if has_arg(args, &["forget", "prune", "delete", "rm", "purge"]) { return refuse("deleting backups is not allowed"); } return Ok(()); }
        "rclone" => { if has_arg(args, &["delete", "deletefile", "purge", "sync", "rmdir", "rmdirs", "cleanup", "move", "moveto", "dedupe"]) { return refuse("rclone deleting/syncing commands are not allowed"); } return Ok(()); }
        "gio" => { if has_arg(args, &["remove"]) || (has_arg(args, &["trash"]) && has_flag(args, &["empty"], &[])) { return refuse("removing files or emptying the trash is not allowed"); } return Ok(()); }
        "kioclient" | "kioclient5" | "kioclient6" => { if has_arg(args, &["del", "delete"]) { return refuse("deleting files is not allowed"); } return Ok(()); }
        "rsync" => {
            if args.iter().any(|a| a.t.starts_with("--delete") || a.t == "--del" || a.t == "--remove-source-files" || a.t == "--remove-sent-files") { return refuse("rsync --delete / --remove-source-files delete files, which is not allowed"); }
            return Ok(());
        }
        "cp" | "mv" | "install" | "ln" | "rename" => return check_placing(b, args, ctx),
        "tee" => {
            if has_flag(args, &["append"], &['a']) { return Ok(()); }
            for a in args.iter().filter(|a| !a.t.starts_with('-') || a.t == "-") { if a.t != "-" { check_overwrite_target(a, ctx, true)?; } }
            return Ok(());
        }
        "curl" | "wget" => {
            let mut it = args.iter();
            while let Some(a) = it.next() {
                let out = if b == "curl" && (a.t == "-o" || a.t == "--output") { it.next() } else if b == "wget" && (a.t == "-O" || a.t == "--output-document") { it.next() } else { None };
                if let Some(o) = out { if o.t != "-" && o.t != "/dev/null" && exists(ctx, o)? { return refuse(format!("{} would overwrite the existing file {}; pick a new name", b, o.t)); } }
            }
            return Ok(());
        }
        _ => {}
    }

    if SHELLS.contains(&b) {
        let mut i = 0;
        let mut script_file: Option<&Word> = None;
        while i < args.len() {
            let a = &args[i].t;
            if a.starts_with('-') && !a.starts_with("--") && a.contains('c') && a.chars().skip(1).all(|c| c.is_ascii_alphabetic()) {
                let Some(s) = args.get(i + 1) else { return Ok(()) };
                if s.dynamic { return refuse("a shell command built from a variable or substitution cannot be checked"); }
                return check_script(&s.t, ctx, depth + 1);
            }
            if !a.starts_with('-') { script_file = Some(&args[i]); break; }
            i += 1;
        }
        if let Some(f) = script_file { return check_script_file(f, ctx, depth); }
        for h in heredocs { check_script(h, ctx, depth + 1)?; }
        for s in herestrings { check_script(&s.t, ctx, depth + 1)?; }
        if piped_in && herestrings.is_empty() && heredocs.is_empty() { return refuse("a shell reading commands from a pipe cannot be checked; write the commands out"); }
        return Ok(());
    }

    if INTERPRETERS.iter().any(|i| b == *i || (b.starts_with(i) && b[i.len()..].chars().all(|c| c.is_ascii_digit() || c == '.'))) {
        let mut found_source = false;
        let mut text = String::new();
        for a in args {
            if a.dynamic { text.push_str(&a.t); text.push(' '); continue; }
            text.push_str(&a.t);
            text.push('\n');
            if let Some(content) = read_small_file(ctx, a) { found_source = true; text.push_str(&content); text.push('\n'); }
        }
        for h in heredocs { text.push_str(h); found_source = true; }
        for s in herestrings { text.push_str(&s.t); found_source = true; }
        if let Some(why) = interpreter_destructive(&text) { return refuse(why); }
        if piped_in && !found_source && (args.is_empty() || args.iter().all(|a| a.t == "-")) { return refuse("an interpreter reading its program from a pipe cannot be checked; write the code out"); }
        return Ok(());
    }

    // a script run by path
    if prog.t.contains('/') {
        if let Some(content) = read_small_file(ctx, prog) {
            let first = content.lines().next().unwrap_or("");
            if first.starts_with("#!") {
                if SHELLS.iter().any(|s| first.contains(s)) { return check_script(&content, ctx, depth + 1); }
                if let Some(why) = interpreter_destructive(&content) { return refuse(why); }
            }
        }
    }

    if WRAPPERS.contains(&b) {
        let limit = words.len().min(14);
        for i in 1..limit {
            check_words_suffix(&words[i..], piped_in, ctx, depth)?;
        }
    }
    Ok(())
}

fn check_words_suffix(words: &[Word], piped_in: bool, ctx: &mut Ctx, depth: u8) -> Result<(), String> {
    if depth > MAX_DEPTH { return refuse("commands nested too deeply to check"); }
    let mut w = words;
    while w.first().is_some_and(is_assignment) { w = &w[1..]; }
    if w.is_empty() || w[0].dynamic && w.len() == 1 { return Ok(()); }
    // an argument of the wrapper that is not itself a program (a number, a host, {}) is simply judged as an unknown program, which is fine
    if w[0].dynamic { return Ok(()); }
    check_words(w, piped_in, ctx, depth + 1, &[], &[])
}

fn read_small_file(ctx: &Ctx, w: &Word) -> Option<String> {
    if w.dynamic || w.t.starts_with('-') { return None; }
    let p = ctx.resolve(&w.t)?;
    let md = std::fs::metadata(&p).ok()?;
    if !md.is_file() || md.len() > 262_144 { return None; }
    String::from_utf8(std::fs::read(&p).ok()?).ok()
}

fn check_script_file(f: &Word, ctx: &mut Ctx, depth: u8) -> Result<(), String> {
    match read_small_file(ctx, f) { Some(text) => check_script(&text, ctx, depth + 1), None => Ok(()) }
}

fn check_find(args: &[Word], ctx: &mut Ctx, depth: u8) -> Result<(), String> {
    for (i, a) in args.iter().enumerate() {
        match a.t.as_str() {
            "-delete" => return refuse("find -delete deletes files, which is not allowed"),
            "-fprint" | "-fprint0" | "-fprintf" | "-fls" => return refuse("find writing its output to a file can overwrite it; redirect to a new file instead"),
            "-exec" | "-execdir" | "-ok" | "-okdir" | "-x" | "-X" | "--exec" | "--exec-batch" => {
                let rest: Vec<Word> = args[i + 1..].iter().take_while(|w| w.t != ";" && w.t != "+").cloned().collect();
                if !rest.is_empty() { check_words(&rest, false, ctx, depth + 1, &[], &[])?; }
            }
            _ => {}
        }
    }
    Ok(())
}

fn check_git(args: &[Word]) -> Result<(), String> {
    let mut i = 0;
    while i < args.len() {
        let a = args[i].t.as_str();
        if matches!(a, "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--exec-path") { i += 2; } else if a.starts_with('-') { i += 1; } else { break; }
    }
    let Some(sub) = args.get(i).map(|w| w.t.as_str()) else { return Ok(()) };
    let rest = &args[i + 1..];
    let any = |long: &[&str], short: &[char]| has_flag(rest, long, short);
    let no = |what: &str| -> Result<(), String> { refuse(format!("git {what} discards or destroys work, which is not allowed")) };
    match sub {
        "clean" => if any(&["dry-run"], &['n']) { Ok(()) } else { no("clean") },
        "reset" => if any(&["hard", "merge"], &[]) { no("reset --hard") } else { Ok(()) },
        "checkout" => if any(&["force", "ours", "theirs"], &['f']) || has_arg(rest, &["--", "."]) { no("checkout over changes") } else { Ok(()) },
        "restore" => if any(&["staged"], &['S']) && !any(&["worktree"], &['W', 'p']) { Ok(()) } else { no("restore") },
        "switch" => if any(&["discard-changes", "force"], &['f']) { no("switch --discard-changes") } else { Ok(()) },
        "push" => {
            if any(&["force", "force-with-lease", "force-if-includes", "delete", "mirror", "prune"], &['f', 'd']) || rest.iter().any(|a| a.t.starts_with('+') || a.t.starts_with(':')) { return refuse("git push --force / delete rewrites or removes remote history, which is not allowed"); }
            Ok(())
        }
        "branch" => if any(&["delete", "force"], &['d', 'D', 'f', 'M']) { no("branch deletion") } else { Ok(()) },
        "tag" => if any(&["delete", "force"], &['d', 'f']) { no("tag deletion") } else { Ok(()) },
        "stash" => if has_arg(rest, &["drop", "clear"]) { no("stash drop") } else { Ok(()) },
        "reflog" => if has_arg(rest, &["expire", "delete"]) { no("reflog expire") } else { Ok(()) },
        "gc" => if rest.iter().any(|a| a.t.starts_with("--prune")) { no("gc --prune") } else { Ok(()) },
        "prune" | "filter-branch" | "filter-repo" | "rm" => no(sub),
        "update-ref" => if any(&[], &['d']) { no("update-ref -d") } else { Ok(()) },
        "worktree" => if has_arg(rest, &["remove", "prune"]) { no("worktree remove") } else { Ok(()) },
        "remote" => if has_arg(rest, &["remove", "rm", "prune"]) { no("remote remove") } else { Ok(()) },
        "submodule" => if has_arg(rest, &["deinit"]) { no("submodule deinit") } else { Ok(()) },
        _ => Ok(()),
    }
}

fn check_containers(b: &str, args: &[Word]) -> Result<(), String> {
    let first: Vec<&str> = args.iter().filter(|a| !a.t.starts_with('-')).map(|a| a.t.as_str()).take(3).collect();
    let verb = first.first().copied().unwrap_or("");
    let second = first.get(1).copied().unwrap_or("");
    let bad = match b {
        "kubectl" | "helm" => matches!(verb, "delete" | "uninstall" | "drain" | "replace"),
        "docker-compose" | "podman-compose" => matches!(verb, "rm" | "kill") || (verb == "down" && args.iter().any(|a| a.t == "-v" || a.t == "--volumes" || a.t == "--rmi")),
        _ => {
            matches!(verb, "rm" | "rmi" | "prune" | "kill")
                || (matches!(verb, "volume" | "system" | "container" | "image" | "network" | "builder" | "buildx" | "pod" | "manifest" | "secret" | "config" | "node" | "service" | "stack") && matches!(second, "rm" | "remove" | "prune" | "delete"))
                || (verb == "compose" && (matches!(second, "rm" | "kill") || (second == "down" && args.iter().any(|a| a.t == "-v" || a.t == "--volumes" || a.t == "--rmi"))))
        }
    };
    if bad { refuse(format!("{b} {verb} {second} deletes containers, images or volumes, which is not allowed")) } else { Ok(()) }
}

/// `cp`, `mv`, `install`, `ln`: refused when a destination already exists (that would replace a file).
fn check_placing(b: &str, args: &[Word], ctx: &Ctx) -> Result<(), String> {
    if b == "rename" { return refuse("rename can overwrite files; use mv to a new name"); }
    if has_flag(args, &["no-clobber"], &['n']) && b != "ln" && b != "install" { return Ok(()); }
    if b == "ln" && !has_flag(args, &["force"], &['f']) { return Ok(()); }
    let mut pos: Vec<&Word> = vec![];
    let mut target_dir: Option<&Word> = None;
    let mut no_target_dir = false;
    let mut i = 0;
    let mut opts_done = false;
    while i < args.len() {
        let a = &args[i];
        if !opts_done && a.t == "--" { opts_done = true; }
        else if !opts_done && (a.t == "-t" || a.t == "--target-directory") { target_dir = args.get(i + 1); i += 1; }
        else if !opts_done && a.t.starts_with("--target-directory=") { return check_dest_dir(&Word { t: a.t["--target-directory=".len()..].to_string(), dynamic: a.dynamic }, &args.iter().filter(|x| !x.t.starts_with('-')).collect::<Vec<_>>(), ctx); }
        else if !opts_done && (a.t == "-T" || a.t == "--no-target-directory") { no_target_dir = true; }
        else if !opts_done && a.t.starts_with('-') && a.t.len() > 1 { if matches!(a.t.as_str(), "-S" | "--suffix" | "-m" | "--mode" | "-o" | "--owner" | "-g" | "--group") { i += 1; } }
        else { pos.push(a); }
        i += 1;
    }
    if let Some(td) = target_dir { return check_dest_dir(td, &pos, ctx); }
    if pos.len() < 2 { return Ok(()); }
    let dest = *pos.last().unwrap();
    let srcs = &pos[..pos.len() - 1];
    if dest.dynamic { return refuse("the destination is built from a variable and cannot be checked; use a plain path"); }
    let dest_path = match ctx.resolve(&dest.t) { Some(p) => p, None => return refuse(format!("cannot tell which folder {} is in; use an absolute path", dest.t)) };
    if !no_target_dir && dest_path.is_dir() { return check_dest_dir(dest, srcs, ctx); }
    if std::fs::symlink_metadata(&dest_path).is_ok() { return refuse(format!("{b} would replace the existing file {}. Use a new name or edit it in place", dest_path.display())); }
    Ok(())
}

fn check_dest_dir(dir: &Word, srcs: &[&Word], ctx: &Ctx) -> Result<(), String> {
    if dir.dynamic { return refuse("the destination folder is built from a variable and cannot be checked; use a plain path"); }
    let Some(d) = ctx.resolve(&dir.t) else { return refuse(format!("cannot tell which folder {} is in; use an absolute path", dir.t)) };
    for s in srcs {
        let Some(name) = Path::new(&s.t).file_name() else { continue };
        let target = d.join(name);
        if std::fs::symlink_metadata(&target).is_ok() { return refuse(format!("it would replace the existing file {}. Use a new name", target.display())); }
    }
    Ok(())
}

/// Deleting through a scripting language: its own file APIs, or a shell command spelled inside it.
fn interpreter_destructive(text: &str) -> Option<String> {
    const CALLS: &[&str] = &[
        "os.remove", "os.unlink", "os.rmdir", "os.removedirs", "shutil.rmtree", "rmtree", "remove_tree", "send2trash", "unlinkSync", "rmSync", "rmdirSync", "fs.rm", "fs.unlink", "fs.rmdir",
        "promises.rm", "promises.unlink", "rimraf", "fse.remove", "removeSync", "emptyDir", "File.delete", "File.unlink", "FileUtils.rm", "FileUtils.remove", "Dir.rmdir", "Dir.delete", "Dir.unlink",
        ".unlink(", "unlink(", "rmdir(", ".rmdir(", "RemoveAll", "os.Remove", "remove_dir", "remove_file", "truncate", "os.truncate", "File::Path", "remove_tree", "rmtree",
    ];
    for c in CALLS { if text.contains(c) { return Some(format!("the script uses {c}, which deletes or empties files; deleting is not allowed")); } }
    for tok in text.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'))) {
        let t = tok.rsplit('/').next().unwrap_or(tok);
        if matches!(t, "rm" | "rmdir" | "unlink" | "shred" | "srm" | "-delete" | "mkfs" | "wipefs" | "trash-empty") { return Some(format!("the script runs or mentions `{t}`, which deletes files; deleting is not allowed")); }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let w = d.path().join("ws");
        std::fs::create_dir_all(w.join("sub")).unwrap();
        std::fs::write(w.join("old.txt"), "x").unwrap();
        std::fs::write(w.join("sub/inner.txt"), "x").unwrap();
        std::fs::write(w.join("clean.sh"), "#!/bin/bash\nrm -rf build\n").unwrap();
        std::fs::write(w.join("nuke.py"), "import shutil\nshutil.rmtree('x')\n").unwrap();
        std::fs::write(w.join("fine.sh"), "#!/bin/bash\necho hi\n").unwrap();
        (d, w)
    }
    fn bash(cmd: &str, w: &Path) -> Option<String> { check("Bash", &json!({"command": cmd}), Path::new("/home/u"), w) }

    #[test]
    fn destructive_commands_are_refused() {
        let (_d, w) = tmp();
        let bad = [
            "rm x", "rm -rf /", "rm -rf ~/projects", "/bin/rm x", "\\rm x", "r\"\"m x", "'rm' x", "command rm x", "env FOO=1 rm x", "nohup rm x &", "timeout 5 rm x", "nice -n 10 rm x",
            "sudo_is_elsewhere; rmdir d", "rmdir d", "unlink old.txt", "shred -u old.txt", "srm old.txt", "ls && rm old.txt", "ls || rm x", "ls; rm x", "ls | xargs rm", "echo x | xargs -I{} rm {}",
            "find . -name '*.o' -delete", "find . -exec rm {} \\;", "find . -type f -exec rm -f {} +", "find . -execdir /bin/rm {} ;", "find . -fprint out.txt",
            "git clean -fd", "git clean -fdx", "git -C repo clean -f", "git reset --hard", "git reset --hard HEAD~1", "git checkout -- file", "git checkout .", "git checkout -f main", "git checkout HEAD -- src",
            "git restore file", "git restore --worktree --staged f", "git restore -p", "git push --force", "git push -f origin main", "git push origin +main", "git push origin :branch", "git push --force-with-lease",
            "git push origin --delete feature", "git branch -D feat", "git branch -d feat", "git branch --delete x", "git tag -d v1", "git stash drop", "git stash clear", "git reflog expire --all", "git gc --prune=now",
            "git rm file", "git filter-branch x", "git worktree remove w", "git remote remove origin", "git switch --discard-changes main", "git update-ref -d refs/heads/x",
            "rsync -a --delete src/ dst/", "rsync -av --delete-after a b", "rsync --remove-source-files a b", "truncate -s 0 old.txt", "dd if=/dev/zero of=/dev/sda", "dd if=a of=b", "dd of=old.txt if=/dev/null2",
            "echo hi > old.txt", "echo hi >old.txt", "cat a >| old.txt", "echo hi &> old.txt", "echo hi 2> old.txt", "cmd > $OUT", "echo x > /dev/sda", "ls >& old.txt", "echo hi | tee old.txt",
            "mv new.txt old.txt", "cp new.txt old.txt", "cp -r a sub/../old.txt", "cp old.txt sub/inner.txt", "cp -t sub old.txt inner.txt", "cp --target-directory=sub x inner.txt", "install -m 755 a old.txt", "ln -sf a old.txt", "cp a $DEST",
            "mkfs.ext4 /dev/sdb1", "mkfs -t ext4 /dev/sdb", "wipefs -a /dev/sda", "fdisk /dev/sda", "parted /dev/sda rm 1", "sgdisk -Z /dev/sda", "blkdiscard /dev/sda",
            "docker rm c", "docker rmi i", "docker volume rm v", "docker volume prune", "docker system prune -a", "podman system prune", "docker container rm x", "docker compose down -v", "docker compose rm", "docker image prune", "docker network prune", "kubectl delete pod x",
            "systemctl disable foo", "systemctl --user disable foo.service", "systemctl --user mask foo", "journalctl --vacuum-time=1d", "snapper delete 5", "btrfs subvolume delete /x", "restic forget abc", "rclone purge remote:", "trash-empty", "gio trash --empty",
            "python3 -c \"import os; os.remove('f')\"", "python -c 'import shutil; shutil.rmtree(\"d\")'", "python3 -c \"from pathlib import Path; Path('f').unlink()\"", "node -e \"require('fs').rmSync('x',{recursive:true})\"",
            "node -e 'fs.unlinkSync(\"a\")'", "perl -e 'unlink \"a\"'", "perl -e 'unlink glob q(*.o)'", "ruby -e 'File.delete(\"a\")'", "ruby -e 'FileUtils.rm_rf(\"a\")'", "php -r 'unlink(\"a\");'", "python3 -c \"import os; os.system('rm -rf x')\"",
            "python3 - <<EOF\nimport os\nos.remove('x')\nEOF", "python3 - <<'EOF'\nimport shutil\nshutil.rmtree('x')\nEOF", "bash -c 'rm x'", "sh -c \"ls; rm x\"", "bash -lc 'echo ok && rm x'", "bash -c \"bash -c 'rm x'\"", "zsh -c 'rm x'",
            "echo $(rm x)", "echo `rm x`", "ls <(rm x)", "echo \"$(rm x)\"", "x=$(rm y)", "( rm x )", "{ rm x; }", "if true; then rm x; fi", "for f in a b; do rm $f; done", "while read l; do rm \"$l\"; done < list", "case a in a) rm x;; esac",
            "echo cm0geA== | base64 -d | sh", "echo 'rm x' | bash", "bash <<< 'rm x'", "bash <<EOF\nrm x\nEOF", "eval 'rm x'", "eval \"$CMD\"", "$CMD x", "cd d && rm x", "FOO=1 rm x", "xargs rm < list",
            "./clean.sh", "bash clean.sh", "sh ./clean.sh", "source clean.sh", ". ./clean.sh", "python3 nuke.py", "curl -o old.txt http://x", "wget -O old.txt http://x", "echo a | python3 -", "ssh host rm x", "busybox rm x", "flock l rm x",
            "cargo build && rm -rf target", "for f in a b; do cp $f $f.bak; done", "echo ok\nrm x", "echo ok \\\n && rm x",
        ];
        for c in bad {
            assert!(bash(c, &w).is_some(), "should be refused: {c:?}");
        }
    }

    #[test]
    fn ordinary_work_is_allowed() {
        let (_d, w) = tmp();
        let good = [
            "ls", "ls -la ~", "cat old.txt", "head -n 5 old.txt | wc -l", "cargo build", "cargo build --release 2>&1 | tail -20", "cargo test -- --nocapture", "git status", "git commit -m 'fix; rm things'", "git commit -am \"rm is not run here\"",
            "git add -A && git commit -m x", "git log --oneline | head", "git diff", "git push origin main", "git push", "git pull --rebase", "git checkout -b feature", "git checkout main", "git switch main", "git branch new", "git branch", "git reset HEAD~1", "git reset --soft HEAD~1",
            "git restore --staged f", "git stash", "git stash pop", "git clean -n", "git clean -fdn", "git tag v1", "git merge x", "git rebase main", "git clone https://x/y", "git -C repo status",
            "mkdir -p new/dir", "mkdir x && cd x && touch y", "touch new.txt", "cp old.txt new.txt", "cp -r sub copy", "cp old.txt sub/other.txt", "cp -n old.txt sub/inner.txt", "mv old.txt renamed.txt", "mv old.txt sub/", "mv -n old.txt sub/inner.txt", "ln -s old.txt link", "cp a ~/newfile.txt",
            "echo hi > new.txt", "echo hi >> old.txt", "echo hi > /dev/null", "ls 2>/dev/null", "cargo build 2>&1", "cmd > out.log 2>&1", "echo hi | tee -a old.txt", "echo hi | tee brandnew.txt", "ls >&2", "cat old.txt > /dev/stderr", "sort < old.txt > sorted.txt",
            "npm install", "npm run build", "npm install --save left-pad", "pip install requests", "python3 script.py", "python3 -c 'print(1)'", "python3 fine.py", "node -e 'console.log(1)'", "node app.js", "perl -e 'print 1'", "python3 - <<EOF\nprint('hi')\nEOF",
            "systemctl --user restart megatron-bot", "systemctl --user status foo", "systemctl --user start x", "systemctl --user daemon-reload", "systemctl --user enable foo", "journalctl --user -u foo | tail", "docker ps", "docker run --rm alpine ls", "docker build -t x .", "docker compose up -d", "docker compose down", "docker logs c",
            "sed -i 's/a/b/' old.txt", "sed -n 1,5p old.txt", "grep -r foo .", "grep -rn 'rm -rf' .", "find . -name '*.rs'", "find . -type f -exec grep -l foo {} +", "find . -exec ls {} \\;", "echo rm", "echo 'rm -rf /'", "printf '%s\\n' unlink", "dd if=a.img bs=1M | head -c 10", "dd if=/dev/zero of=/dev/null count=1",
            "rsync -av src/ dst/", "curl -s https://example.com", "curl -o fresh.html https://example.com", "wget -O new.bin https://x", "tar czf out.tgz dir", "unzip a.zip", "bash fine.sh", "./fine.sh", "bash -c 'echo hi'", "bash -c 'ls && pwd'", "sh -c \"cargo build\"",
            "for f in *.txt; do echo $f; done", "x=$(date); echo $x", "echo $(date)", "echo `whoami`", "export A=1; cargo build", "A=1 cargo build", "env FOO=1 cargo test", "timeout 60 cargo test", "nohup cargo build &", "time cargo build", "xargs -n1 echo < old.txt", "cat old.txt | xargs echo",
            "xdg-open https://x", "gio open file", "snapper list", "btrfs subvolume list /", "trash-put old.txt", "make", "make test", "cargo clean --help", "ls # rm x", "echo done", "cd sub && ls", "cd .. && pwd", "pwd; whoami; date", "which rm", "man rm", "type rm", 
        ];
        for c in good {
            let r = bash(c, &w);
            assert!(r.is_none(), "should be allowed: {c:?} -> {r:?}");
        }
    }

    #[test]
    fn write_replacing_an_existing_file_is_refused_but_new_files_and_edits_pass() {
        let (_d, w) = tmp();
        let home = Path::new("/home/u");
        assert!(check("Write", &json!({"file_path": w.join("old.txt"), "content": "x"}), home, &w).is_some());
        assert!(check("Write", &json!({"file_path": "old.txt", "content": "x"}), home, &w).is_some());
        assert!(check("Write", &json!({"file_path": w.join("brand-new.txt"), "content": "x"}), home, &w).is_none());
        assert!(check("Edit", &json!({"file_path": w.join("old.txt"), "old_string": "x", "new_string": "y"}), home, &w).is_none());
        assert!(check("MultiEdit", &json!({"file_path": w.join("old.txt"), "edits": []}), home, &w).is_none());
        assert!(check("Read", &json!({"file_path": w.join("old.txt")}), home, &w).is_none());
        assert!(check("NotebookEdit", &json!({"notebook_path": "a.ipynb", "edit_mode": "delete"}), home, &w).is_some());
        assert!(check("NotebookEdit", &json!({"notebook_path": "a.ipynb", "edit_mode": "replace"}), home, &w).is_none());
        assert!(check("WebFetch", &json!({"url": "https://x"}), home, &w).is_none());
    }

    #[test]
    fn the_reason_is_short_and_names_the_problem() {
        let (_d, w) = tmp();
        let why = bash("rm -rf build", &w).unwrap();
        assert!(why.contains("rm") && why.len() < 200, "{why}");
    }
}
