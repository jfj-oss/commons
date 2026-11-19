//! One manual smoke run of the REAL Claude Code CLI with a harmless read-only task. `#[ignore]`: it spends a little usage and
//! is never part of `cargo test`. Run: `cargo test -p commons-bot --test assistant_smoke -- --ignored --nocapture`.
//! Nothing is asked: the destruction guard decides, and a read-only task needs no more.
use commons_bot::assistant::config::AssistConfig;
use commons_bot::assistant::tools::RealSys;
use commons_bot::assistant::{Assistant, RoomFacts};
use commons_bot::bot::{Chat, Job, JobKind, Line};
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
        text: std::env::var("SMOKE_TASK").unwrap_or_else(|_| "/claude How much free disk space is there on this computer? Run one read-only df command and report the numbers briefly. Change nothing.".into()) };
    let (a2, mut c2) = (a.clone(), chat.clone());
    let worker = std::thread::spawn(move || a2.handle(&mut c2, &job, &NoLlm, &NoSearch, None));
    worker.join().unwrap();
    println!("[smoke] log:\n{}", std::fs::read_to_string(a.log_path()).unwrap_or_default());
}
