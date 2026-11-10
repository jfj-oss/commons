//! The assistant room's gating through the real client and a local relay: membership comes from the encrypted group.
mod common;
use common::*;
use commons_bot::assistant::config::AssistConfig;
use commons_bot::assistant::tools::RealSys;
use commons_bot::assistant::Assistant;
use commons_bot::bot::{Bot, Config, JobKind};
use commons_bot::llm::{DeepSeek, Ollama};
use std::sync::Arc;
use std::time::Duration;

#[test]
fn membership_decides_whether_the_room_is_an_assistant_room() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut oracle = Peer::new(&net, "Oracle");
    let (_cid, gen) = community_with(&net, &mut alice, &mut [&mut oracle]);

    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = AssistConfig::new(&alice.acct(), tmp.path(), &tmp.path().join("data"));
    cfg.use_systemd_run = false;
    let ollama = fake_ollama(&["qwen3.5:9b"], "hi");
    let key = tmp.path().join("key");
    std::fs::write(&key, "k").unwrap();
    let bot = Bot::new(
        Config::default(),
        Box::new(Ollama::new(&ollama.url(), None, Duration::from_secs(5))),
        Box::new(DeepSeek::new("http://127.0.0.1:1", key, "deepseek-chat", Duration::from_secs(5))),
        Arc::new(DeadSearch), Arc::new(DeadStudio), Arc::new(NoGpu), tmp.path().join("state.json"),
    ).with_assistant(Arc::new(Assistant::new(cfg, Arc::new(RealSys))));
    assert!(bot.poll(&mut oracle.c).is_empty());

    // only the two of us: the phrase switches it on
    alice.c.send_text(&gen, "@Oracle assistant here", None).unwrap();
    settle(&mut [&mut alice, &mut oracle]);
    let jobs = bot.poll(&mut oracle.c);
    assert_eq!(jobs.len(), 1);
    assert!(matches!(&jobs[0].kind, JobKind::AssistantNotice(t) if t.contains("Assistant mode is on")), "{:?}", jobs[0].kind);

    // everything the owner writes now goes to the assistant, mention or not
    alice.c.send_text(&gen, "how much disk is free", None).unwrap();
    settle(&mut [&mut alice, &mut oracle]);
    let jobs = bot.poll(&mut oracle.c);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].kind, JobKind::Assistant);

    // a third person joins: the room stops being an assistant room, with one notice, and the ordinary bot is back
    let mut bob = Peer::new(&net, "bob");
    let link = alice.c.create_invite(&_cid, None, 3600).unwrap();
    bob.c.join_with_link(&link).unwrap();
    settle(&mut [&mut alice, &mut oracle, &mut bob]);
    let jobs = bot.poll(&mut oracle.c);
    assert_eq!(jobs.iter().filter(|j| matches!(&j.kind, JobKind::AssistantNotice(t) if t.contains("no longer an assistant room"))).count(), 1, "{jobs:?}");
    bob.c.send_text(&gen, "@Oracle hello there", None).unwrap();
    alice.c.send_text(&gen, "how much disk is free", None).unwrap();
    settle(&mut [&mut alice, &mut oracle, &mut bob]);
    let jobs = bot.poll(&mut oracle.c);
    assert!(jobs.iter().all(|j| !matches!(j.kind, JobKind::Assistant | JobKind::AssistantNotice(_))), "{jobs:?}");
    assert!(jobs.iter().any(|j| matches!(j.kind, JobKind::Answer { .. })), "bob's mention is answered the ordinary way: {jobs:?}");
}
