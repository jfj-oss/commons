//! The two model clients against fake servers. Nothing here touches a real Ollama or the real DeepSeek API.
mod common;
use common::*;
use commons_bot::llm::{AskError, DeepSeek, Llm, Msg, Ollama, ToolMode};
use std::time::Duration;

fn history() -> Vec<Msg> {
    vec![Msg::User("alice: @Oracle what's a good pasta recipe".into())]
}

#[test]
fn ollama_uses_the_owners_model_and_sends_the_room_as_context() {
    let fake = fake_ollama(&["llama3.2:3b", "qwen3.5:9b", "nomic-embed-text:latest"], "Try aglio e olio.");
    let ollama = Ollama::new(&fake.url(), None, Duration::from_secs(5));
    assert_eq!(ollama.model().unwrap().as_deref(), Some("qwen3.5:9b"));
    let answer = ollama.complete("You are Oracle.", &history(), &[]).unwrap();
    assert_eq!(answer.content, "Try aglio e olio.");
    assert!(answer.tool_calls.is_empty());

    let chat = fake.requests().into_iter().find(|r| r.path == "/api/chat").expect("a chat request");
    let body = chat.json();
    assert_eq!(body["model"], "qwen3.5:9b");
    assert_eq!(body["stream"], false);
    assert_eq!(body["messages"][0]["role"], "system");
    assert!(body["messages"][0]["content"].as_str().unwrap().contains("Oracle"));
    assert!(body["messages"][1]["content"].as_str().unwrap().contains("pasta recipe"));
    // Every request asks Ollama to keep the model for five minutes; the GPU watcher can free it sooner.
    assert_eq!(body["keep_alive"], "5m");
}

#[test]
fn a_configured_model_wins_when_it_is_installed() {
    let fake = fake_ollama(&["llama3.2:3b", "qwen3.5:9b"], "ok");
    let ollama = Ollama::new(&fake.url(), Some("llama3.2:3b"), Duration::from_secs(5));
    assert_eq!(ollama.model().unwrap().as_deref(), Some("llama3.2:3b"));
}

#[test]
fn ollama_down_is_reported_as_unavailable() {
    let fake = fake_ollama(&["qwen3.5:9b"], "never");
    let url = fake.url();
    drop(fake);
    let ollama = Ollama::new(&url, None, Duration::from_millis(800));
    match ollama.complete("sys", &history(), &[]) {
        Err(AskError::Unavailable(m)) => assert!(m.contains("unreachable"), "{m}"),
        other => panic!("expected unavailable, got {other:?}"),
    }
}

#[test]
fn an_ollama_with_only_embedding_models_says_so() {
    let fake = fake_ollama(&["nomic-embed-text:latest"], "unused");
    let ollama = Ollama::new(&fake.url(), None, Duration::from_secs(5));
    assert_eq!(ollama.model().unwrap(), None);
    match ollama.complete("sys", &history(), &[]) {
        Err(AskError::Failed(m)) => assert!(m.contains("no chat model"), "{m}"),
        other => panic!("expected a failed answer, got {other:?}"),
    }
    assert_eq!(fake.hits("/api/chat"), 0);
}

#[test]
fn a_slow_ollama_times_out_without_hanging_the_caller() {
    let fake = FakeHttp::start(|r| match r.path.as_str() {
        "/api/tags" => (200, r#"{"models":[{"name":"qwen3.5:9b"}]}"#.into()),
        "/api/show" => (200, r#"{"capabilities":["completion","tools"]}"#.into()),
        "/api/chat" => { std::thread::sleep(Duration::from_secs(3)); (200, r#"{"message":{"content":"late"}}"#.into()) }
        _ => (404, "{}".into()),
    });
    let ollama = Ollama::new(&fake.url(), None, Duration::from_millis(400));
    let started = std::time::Instant::now();
    match ollama.complete("sys", &history(), &[]) {
        Err(AskError::Unavailable(_)) => {}
        other => panic!("expected a timeout, got {other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(2), "the timeout was not applied");
}

#[test]
fn a_model_without_tool_support_gets_the_text_protocol() {
    let with = fake_ollama(&["qwen3.5:9b"], "ok");
    let without = fake_ollama_without_tools(&["qwen3.5:9b"], "ok");
    let a = Ollama::new(&with.url(), None, Duration::from_secs(5));
    let b = Ollama::new(&without.url(), None, Duration::from_secs(5));
    assert_eq!(a.tool_mode(), ToolMode::FunctionCalling);
    assert_eq!(b.tool_mode(), ToolMode::TextProtocol);
}

#[test]
fn unloading_sends_keep_alive_zero_for_the_chosen_model() {
    let fake = fake_ollama(&["qwen3.5:9b"], "ok");
    let ollama = Ollama::new(&fake.url(), None, Duration::from_secs(5));
    ollama.unload().unwrap();
    let unload = fake.requests().into_iter().find(|r| r.path == "/api/generate").expect("an unload request");
    let body = unload.json();
    assert_eq!(body["model"], "qwen3.5:9b");
    assert_eq!(body["keep_alive"], 0, "keep_alive 0 tells Ollama to free the VRAM at once");
}

#[test]
fn deepseek_gets_the_key_and_its_answer_is_used() {
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("key");
    std::fs::write(&key, "test-key-123\n").unwrap();
    let fake = fake_deepseek("Use less garlic next time.");
    let ds = DeepSeek::new(&fake.url(), key, "deepseek-chat", Duration::from_secs(5));
    assert_eq!(ds.complete("You are Oracle.", &history(), &[]).unwrap().content, "Use less garlic next time.");
    let req = fake.requests().into_iter().find(|r| r.path == "/chat/completions").expect("a completion request");
    assert_eq!(req.method, "POST");
    assert_eq!(req.header("authorization"), Some("Bearer test-key-123"));
    let body = req.json();
    assert_eq!(body["model"], "deepseek-chat");
    assert_eq!(body["messages"][0]["role"], "system");
}

#[test]
fn a_rejected_deepseek_key_is_a_clear_failure() {
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("key");
    std::fs::write(&key, "bad-key").unwrap();
    let fake = FakeHttp::start(|_| (401, r#"{"error":{"message":"invalid key"}}"#.into()));
    let ds = DeepSeek::new(&fake.url(), key, "deepseek-chat", Duration::from_secs(5));
    match ds.complete("sys", &history(), &[]) {
        Err(AskError::Failed(m)) => assert!(m.contains("401"), "{m}"),
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[test]
fn a_deepseek_that_is_down_is_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("key");
    std::fs::write(&key, "k").unwrap();
    let fake = fake_deepseek("x");
    let url = fake.url();
    drop(fake);
    let ds = DeepSeek::new(&url, key, "deepseek-chat", Duration::from_millis(800));
    match ds.complete("sys", &history(), &[]) {
        Err(AskError::Unavailable(_)) => {}
        other => panic!("expected unavailable, got {other:?}"),
    }
}
