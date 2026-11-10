//! One manual smoke run of the REAL Claude Code CLI with a harmless read-only task. `#[ignore]`: it spends a little usage and
//! is never part of `cargo test`. Run: `cargo test -p commons-bot --test assistant_smoke -- --ignored --nocapture`.
//! Approvals are answered by this test: only a plain `df`/`free` style command is approved, everything else is denied.
use commons_bot::assistant::config::AssistConfig;
use commons_bot::assistant::tools::RealSys;
use commons_bot::assistant::{Assistant, RoomFacts};
use commons_bot::bot::{Chat, Job, JobKind, Line};
use commons_bot::assistant::approvals::{parse_reply, Reply};
use commons_bot::llm::{AskError, Llm, ModelReply, Msg, ToolSpec};
use commons_bot::search::{SearchResult, WebSearch};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Chat0 { lines: Arc<Mutex<Vec<(bool, String)>>> }
impl Chat for Chat0 {
    fn my_account(&self) -> String { "bbbbbbbbbbbb".into() }
    fn history(&self, _g: &str, _l: usize) -> Vec<Line> { vec![] }
    fn typing(&mut self, _g: &str) {}
    fn send(&mut self, _g: &str, t: &str, _r: Option<String>) -> Result<String, String> { println!("[chat] {t}\n"); self.lines.lock().unwrap().push((true, t.into())); Ok("i".into()) }
    fn send_file(&mut self, _g: &str, _n: &str, _m: &str, _d: &[u8], _c: &str) -> Result<String, String> { Err("no".into()) }
}
struct NoLlm;
impl Llm for NoLlm { fn complete(&self, _s: &str, _m: &[Msg], _t: &[ToolSpec]) -> Result<ModelReply, AskError> { Err(AskError::Failed("none".into())) } }
struct NoSearch;
impl WebSearch for NoSearch { fn search(&self, _q: &str) -> Result<Vec<SearchResult>, String> { Err("none".into()) } }

#[test]
#[ignore]
fn real_claude_read_only_disk_question() {
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    let data = home.join("data/agent-tmp/oracle-assistant/smoke-data");
    std::fs::create_dir_all(&data).unwrap();
    let mut cfg = AssistConfig::new("aaaaaaaaaaaa", &home, &data);
    cfg.self_exe = PathBuf::from(env!("CARGO_BIN_EXE_megatron-bot"));
    cfg.approval_timeout = std::time::Duration::from_secs(120);
    let a = Arc::new(Assistant::new(cfg, Arc::new(RealSys)));
    let s: BTreeSet<String> = ["aaaaaaaaaaaa".to_string(), "bbbbbbbbbbbb".to_string()].into_iter().collect();
    let facts = RoomFacts { gid: "smoke".into(), me: "bbbbbbbbbbbb".into(), channel: Some(s.clone()), community: Some(s) };
    a.route(&facts, "Oracle", "aaaaaaaaaaaa", "@Oracle assistant here");
    let chat = Chat0::default();
    let job = Job { gid: "smoke".into(), trigger: "t".into(), from: "aaaaaaaaaaaa".into(), kind: JobKind::Assistant,
        text: std::env::var("SMOKE_TASK").unwrap_or_else(|_| "do: How much free disk space is there on this computer? Run one read-only df command and report the numbers briefly. Change nothing.".into()) };
    let answer_yes = std::env::var("SMOKE_ANSWER").map_or(true, |v| v == "yes");
    let (a2, mut c2) = (a.clone(), chat.clone());
    let worker = std::thread::spawn(move || a2.handle(&mut c2, &job, &NoLlm, &NoSearch, None));
    let mut seen = 0;
    while !worker.is_finished() {
        std::thread::sleep(std::time::Duration::from_millis(300));
        let msgs: Vec<String> = chat.lines.lock().unwrap().iter().map(|l| l.1.clone()).collect();
        for m in msgs.iter().skip(seen) {
            if let Some(rest) = m.strip_prefix("Approval needed [") {
                let code = &rest[..4];
                let cmd = m.split("Command:\n").nth(1).and_then(|c| c.lines().next()).unwrap_or("").to_string();
                println!("[smoke] request text: {m}");
                let ok = answer_yes && m.contains("Tool: Bash") && ["df", "free", "lsblk"].iter().any(|p| cmd == *p || cmd.starts_with(&format!("{p} "))) && !cmd.contains([';', '|', '&', '>', '<', '$', '`']);
                println!("[smoke] request {code}: {cmd:?} -> {}", if ok { "yes" } else { "no" });
                let r = parse_reply(&format!("{} {code}", if ok { "yes" } else { "no" })).unwrap();
                assert!(matches!(r, Reply::Yes(_) | Reply::No(_)));
                a.route(&facts, "Oracle", "aaaaaaaaaaaa", &format!("{} {code}", if ok { "yes" } else { "no" }));
            }
        }
        seen = msgs.len();
    }
    worker.join().unwrap();
    println!("[smoke] log:\n{}", std::fs::read_to_string(a.log_path()).unwrap_or_default());
}
