//! End to end through the local relay: a person mentions the bot, the bot asks a (fake) model and answers, encrypted
//! like every other member's message. The real Ollama and DeepSeek services are never contacted.
mod common;
use common::*;
use commons_bot::bot::{Bot, Config, JobKind};
use commons_bot::llm::{DeepSeek, Ollama};
use commons_core::state::MsgBody;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn make_bot(cfg: Config, state: &Path, ollama_url: &str, deepseek_url: &str) -> Bot {
    let key = state.with_extension("deepseek-key");
    std::fs::write(&key, "test-deepseek-key\n").unwrap();
    Bot::new(
        cfg,
        Box::new(Ollama::new(ollama_url, None, Duration::from_secs(5))),
        Box::new(DeepSeek::new(deepseek_url, key, "deepseek-chat", Duration::from_secs(5))),
        Arc::new(DeadSearch),
        Arc::new(DeadStudio),
        Arc::new(NoGpu),
        state,
    )
}

#[test]
fn a_mention_gets_an_encrypted_answer_and_other_chatter_does_not() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut oracle = Peer::new(&net, "Oracle");
    let (_cid, gen) = community_with(&net, &mut alice, &mut [&mut oracle]);

    let ollama = fake_ollama(&["llama3.2:3b", "qwen3.5:9b"], "Try spaghetti aglio e olio.");
    let state = tempfile::tempdir().unwrap();
    let bot = make_bot(Config::default(), &state.path().join("state.json"), &ollama.url(), "http://127.0.0.1:1");
    assert!(bot.poll(&mut oracle.c).is_empty(), "an empty room has nothing to answer");

    // Plain chatter with its name but no @: silence.
    alice.c.send_text(&gen, "pasta is nice, ask Oracle later", None).unwrap();
    settle(&mut [&mut alice, &mut oracle]);
    assert!(bot.poll(&mut oracle.c).is_empty());

    // A mention: exactly one job, with the last messages as context.
    alice.c.send_text(&gen, "@Oracle what's a good pasta recipe?", None).unwrap();
    settle(&mut [&mut alice, &mut oracle]);
    let jobs = bot.poll(&mut oracle.c);
    assert_eq!(jobs.len(), 1, "one mention, one answer");
    assert_eq!(jobs[0].gid, gen);
    assert_eq!(jobs[0].kind, JobKind::Answer { deepseek: false });
    let sent = bot.answer(&mut oracle.c, &jobs[0]).expect("an answer");
    assert!(sent.text.contains("aglio e olio"));

    settle(&mut [&mut alice, &mut oracle]);
    assert!(has_text(&alice, &gen, &sent.text), "alice sees the bot's answer");

    let chat = ollama.requests().into_iter().find(|r| r.path == "/api/chat").expect("the model was asked");
    let body = chat.json();
    let msgs = body["messages"].as_array().unwrap();
    assert!(msgs.len() >= 2 && msgs.len() <= 21, "system prompt plus at most 20 room messages, got {}", msgs.len());
    assert!(body["messages"].as_array().unwrap().iter().any(|m| m["content"].as_str().unwrap_or("").contains("@Oracle what's a good pasta recipe?")));

    // The relay only ever saw ciphertext.
    let disk = relay_bytes(&net);
    assert!(!contains(&disk, "aglio e olio"), "the answer must not be readable on the relay");
    assert!(!contains(&disk, "pasta recipe"), "the question must not be readable on the relay");

    // A reply to the bot's own message is addressed to it even without a mention; its own messages are not.
    alice.c.send_text(&gen, "and a wine?", Some(sent.id.clone())).unwrap();
    settle(&mut [&mut alice, &mut oracle]);
    let jobs = bot.poll(&mut oracle.c);
    assert_eq!(jobs.len(), 1, "a reply to the bot counts");
    let again = bot.answer(&mut oracle.c, &jobs[0]).unwrap();
    settle(&mut [&mut alice, &mut oracle]);
    assert!(has_text(&alice, &gen, &again.text));
    assert!(bot.poll(&mut oracle.c).is_empty(), "the bot does not answer itself");

    // Answers are threaded to the message they answer.
    let answered = alice.c.messages(&gen).into_iter().find(|m| m.id == again.id).unwrap();
    assert_eq!(answered.reply.as_deref(), Some(jobs[0].trigger.as_str()));
    assert_eq!(answered.sender, oracle.acct());
}

#[test]
fn only_an_admin_can_switch_a_room_to_deepseek_and_the_label_shows() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut bob = Peer::new(&net, "bob");
    let mut oracle = Peer::new(&net, "Oracle");
    let (_cid, gen) = community_with(&net, &mut alice, &mut [&mut bob, &mut oracle]);

    let ollama = fake_ollama(&["qwen3.5:9b"], "LOCAL ANSWER");
    let deepseek = fake_deepseek("DEEPSEEK ANSWER");
    let state = tempfile::tempdir().unwrap();
    let path = state.path().join("state.json");
    let mut cfg = Config::default();
    cfg.max_per_minute = 100;
    let bot = make_bot(cfg, &path, &ollama.url(), &deepseek.url());
    bot.poll(&mut oracle.c);

    // A member who is not an admin cannot flip it; and a sentence that merely contains the words is not the phrase.
    bob.c.send_text(&gen, "@Oracle use deepseek", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut oracle]);
    let jobs = bot.poll(&mut oracle.c);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].kind, JobKind::NotAllowed);
    assert!(!bot.deepseek_on(&gen));
    let refused = bot.answer(&mut oracle.c, &jobs[0]).unwrap();
    assert!(refused.text.contains("Only an admin"));

    alice.c.send_text(&gen, "@Oracle use deepseek because I said so", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut oracle]);
    let jobs = bot.poll(&mut oracle.c);
    assert_eq!(jobs[0].kind, JobKind::Answer { deepseek: false }, "not the exact admin phrase");
    assert!(!bot.deepseek_on(&gen));

    // The owner is an admin: the switch takes, and is remembered across a restart of the bot process.
    alice.c.send_text(&gen, "@Oracle use deepseek", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut oracle]);
    let jobs = bot.poll(&mut oracle.c);
    assert_eq!(jobs[0].kind, JobKind::SetDeepSeek { on: true });
    assert!(bot.deepseek_on(&gen));
    bot.answer(&mut oracle.c, &jobs[0]).unwrap();
    let restarted = make_bot(Config::default(), &path, &ollama.url(), &deepseek.url());
    assert!(restarted.deepseek_on(&gen), "the per-room choice survives a restart");

    // A mention in that room goes to DeepSeek, carries the visible label, and does not touch Ollama.
    alice.c.send_text(&gen, "@Oracle best wine with pasta?", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut oracle]);
    let jobs = bot.poll(&mut oracle.c);
    assert_eq!(jobs[0].kind, JobKind::Answer { deepseek: true });
    let sent = bot.answer(&mut oracle.c, &jobs[0]).unwrap();
    assert!(sent.text.contains("DEEPSEEK ANSWER") && sent.text.contains("(DeepSeek, leaves this chat)"), "{}", sent.text);
    assert_eq!(deepseek.hits("/chat/completions"), 1);
    assert_eq!(ollama.hits("/api/chat"), 0, "nothing goes to the local model in DeepSeek mode");

    // Back to local: no label, and Ollama is the one asked.
    alice.c.send_text(&gen, "@Oracle use local", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut oracle]);
    assert_eq!(bot.poll(&mut oracle.c)[0].kind, JobKind::SetDeepSeek { on: false });
    assert!(!bot.deepseek_on(&gen));
    alice.c.send_text(&gen, "@Oracle one more recipe", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut oracle]);
    let jobs = bot.poll(&mut oracle.c);
    let sent = bot.answer(&mut oracle.c, &jobs[0]).unwrap();
    assert!(sent.text.contains("LOCAL ANSWER") && !sent.text.contains("DeepSeek"));
    assert_eq!(ollama.hits("/api/chat"), 1);
    assert_eq!(deepseek.hits("/chat/completions"), 1);
}

#[test]
fn ollama_down_gets_a_polite_message_not_silence() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut oracle = Peer::new(&net, "Oracle");
    let (_cid, gen) = community_with(&net, &mut alice, &mut [&mut oracle]);

    let dead = fake_ollama(&["qwen3.5:9b"], "unused");
    let url = dead.url();
    drop(dead);

    let state = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    cfg.max_per_minute = 100;
    let bot = make_bot(cfg, &state.path().join("state.json"), &url, "http://127.0.0.1:1");
    bot.poll(&mut oracle.c);

    alice.c.send_text(&gen, "@Oracle are you there?", None).unwrap();
    settle(&mut [&mut alice, &mut oracle]);
    let jobs = bot.poll(&mut oracle.c);
    assert_eq!(jobs.len(), 1);
    let sent = bot.answer(&mut oracle.c, &jobs[0]).expect("an apology is still a reply");
    assert!(sent.text.contains("can't reach the local model"), "{}", sent.text);
    settle(&mut [&mut alice, &mut oracle]);
    assert!(has_text(&alice, &gen, &sent.text));
}

#[test]
fn the_post_limit_holds_across_a_burst() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut oracle = Peer::new(&net, "Oracle");
    let (_cid, gen) = community_with(&net, &mut alice, &mut [&mut oracle]);

    let ollama = fake_ollama(&["qwen3.5:9b"], "one answer");
    let state = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    cfg.max_per_minute = 1;
    let bot = make_bot(cfg, &state.path().join("state.json"), &ollama.url(), "http://127.0.0.1:1");
    bot.poll(&mut oracle.c);

    alice.c.send_text(&gen, "@Oracle first question", None).unwrap();
    alice.c.send_text(&gen, "@Oracle second question", None).unwrap();
    settle(&mut [&mut alice, &mut oracle]);
    let jobs = bot.poll(&mut oracle.c);
    assert_eq!(jobs.len(), 2, "both are queued as jobs");
    assert!(bot.answer(&mut oracle.c, &jobs[0]).is_some());
    assert!(bot.answer(&mut oracle.c, &jobs[1]).is_none(), "the minute's second post is refused");
    settle(&mut [&mut alice, &mut oracle]);
    let from_oracle = alice.c.messages(&gen).into_iter().filter(|m| m.sender == oracle.acct() && matches!(m.body, MsgBody::Text(_))).count();
    assert_eq!(from_oracle, 1, "only one answer reached the room");
}

#[test]
fn a_long_answer_is_cut_and_mentions_from_the_window_do_not_repeat_after_a_restart() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut oracle = Peer::new(&net, "Oracle");
    let (_cid, gen) = community_with(&net, &mut alice, &mut [&mut oracle]);

    let long = "word ".repeat(2000);
    let ollama = fake_ollama(&["qwen3.5:9b"], &long);
    let state = tempfile::tempdir().unwrap();
    let path = state.path().join("state.json");
    let mut cfg = Config::default();
    cfg.max_reply_chars = 200;
    cfg.max_per_minute = 100;
    let bot = make_bot(cfg, &path, &ollama.url(), "http://127.0.0.1:1");
    bot.poll(&mut oracle.c);

    alice.c.send_text(&gen, "@Oracle tell me everything", None).unwrap();
    settle(&mut [&mut alice, &mut oracle]);
    let jobs = bot.poll(&mut oracle.c);
    let sent = bot.answer(&mut oracle.c, &jobs[0]).unwrap();
    assert!(sent.text.chars().count() <= 200, "{}", sent.text.chars().count());
    assert!(sent.text.ends_with('…'));
    settle(&mut [&mut alice, &mut oracle]);

    // A new bot process (same state) does not answer the same mention again.
    let again = make_bot(Config::default(), &path, &ollama.url(), "http://127.0.0.1:1");
    assert!(again.poll(&mut oracle.c).is_empty());
    settle(&mut [&mut alice, &mut oracle]);
    let answers = alice.c.messages(&gen).into_iter().filter(|m| m.sender == oracle.acct()).count();
    assert_eq!(answers, 1);
}
