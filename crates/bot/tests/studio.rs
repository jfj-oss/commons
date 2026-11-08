//! Studio: the second tool. A fake Studio server (never the real one), a fake Ollama that asks for `studio_make`, and
//! the ordinary relay + clients. These cover the whole contract: pictures posted as encrypted attachments, the owner
//! and room permissions, the two hard-block rules, the daily cap, a game holding the GPU, and video being owner-only.
mod common;
use common::*;
use commons_bot::bot::{Bot, Chat, Config, Job, JobKind, Line};
use commons_bot::gpu::GpuCheck;
use commons_bot::llm::{DeepSeek, Ollama};
use commons_bot::safety;
use commons_bot::search::WebSearch;
use commons_bot::studio::{HttpStudio, StudioApi};
use commons_core::state::{MsgBody, Role};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------- fake Studio

/// A Studio that accepts one job and finishes it at once. `state` is what `/api/state` says; the result body is fixed
/// bytes (the bot never inspects them). Every request is recorded, so a test can assert what was and was not sent.
fn fake_studio(state: Value, result: &'static str) -> FakeHttp {
    FakeHttp::start(move |r| {
        let path = r.path.split('?').next().unwrap_or("");
        match (r.method.as_str(), path) {
            ("GET", "/api/state") => (200, json!({"ok": true, "game": state["game"], "paused": state["paused"]}).to_string()),
            ("POST", "/api/jobs") => (200, json!({"ok": true, "job": {"id": 1, "status": "queued"}}).to_string()),
            ("GET", "/api/jobs/1") => (200, json!({"ok": true, "job": {"id": 1, "status": "done"}}).to_string()),
            ("GET", "/img/1") | ("GET", "/aud/1") | ("GET", "/vid/1") => (200, result.to_string()),
            _ => (404, json!({"ok": false, "error": "not found"}).to_string()),
        }
    })
}

fn idle_state() -> Value { json!({"game": false, "paused": ""}) }

fn studio_with(server: &FakeHttp, dir: &Path) -> Arc<dyn StudioApi> {
    let token = dir.join("homebase-token");
    std::fs::write(&token, "test-token\n").unwrap();
    Arc::new(HttpStudio::new(&server.url(), &token))
}

// ---------------------------------------------------------------- fake Ollama that asks for Studio

/// Even-numbered completions ask to make one thing; odd-numbered ones answer. Each `answer` call is exactly two
/// completions, so this repeats cleanly whatever the test does.
fn studio_ollama_args(args: Value) -> FakeHttp {
    let calls = Arc::new(AtomicUsize::new(0));
    FakeHttp::start(move |r| match r.path.as_str() {
        "/api/tags" => (200, json!({"models": [{"name": "qwen3.5:9b"}]}).to_string()),
        "/api/show" => (200, json!({"capabilities": ["completion", "tools"]}).to_string()),
        "/api/chat" => {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            let reply = if n % 2 == 0 {
                json!({"role": "assistant", "content": "", "tool_calls": [{"function": {
                    "name": "studio_make", "arguments": args
                }}]})
            } else {
                json!({"role": "assistant", "content": "Aight, done."})
            };
            (200, json!({"message": reply}).to_string())
        }
        "/api/generate" => (200, json!({"done": true}).to_string()),
        _ => (404, "{}".into()),
    })
}

fn studio_ollama(kind: &str, prompt: &str) -> FakeHttp {
    studio_ollama_args(json!({"kind": kind, "prompt": prompt}))
}

// ---------------------------------------------------------------- a chat that only remembers

struct FakeChat {
    role: Role,
    lines: Vec<Line>,
    sent: Vec<String>,
    files: Vec<(String, String, usize, String)>,
}
impl FakeChat {
    fn new(role: Role) -> FakeChat {
        FakeChat {
            role,
            lines: vec![Line { mine: false, name: "alice".into(), text: "@Oracle make me something".into() }],
            sent: Vec::new(),
            files: Vec::new(),
        }
    }
}
impl Chat for FakeChat {
    fn my_account(&self) -> String { "bot".into() }
    fn history(&self, _gid: &str, _limit: usize) -> Vec<Line> { self.lines.clone() }
    fn typing(&mut self, _gid: &str) {}
    fn send(&mut self, _gid: &str, text: &str, _reply: Option<String>) -> Result<String, String> {
        self.sent.push(text.to_string());
        Ok(format!("m{}", self.sent.len()))
    }
    fn send_file(&mut self, _gid: &str, name: &str, mime: &str, data: &[u8], caption: &str) -> Result<String, String> {
        self.files.push((name.to_string(), mime.to_string(), data.len(), caption.to_string()));
        Ok(format!("f{}", self.files.len()))
    }
    fn role_of(&self, _gid: &str, _account: &str) -> Role { self.role }
    fn owner_of(&self, _gid: &str) -> Option<String> { Some("alice".into()) }
}

fn make_bot(
    dir: &Path,
    cfg: Config,
    ollama_url: &str,
    search: Arc<dyn WebSearch>,
    studio: Arc<dyn StudioApi>,
    gpu: Arc<dyn GpuCheck>,
) -> Bot {
    let key = dir.join("deepseek-key");
    std::fs::write(&key, "test-key\n").unwrap();
    Bot::new(
        cfg,
        Box::new(Ollama::new(ollama_url, None, Duration::from_secs(5))),
        Box::new(DeepSeek::new("http://127.0.0.1:1", key, "deepseek-chat", Duration::from_secs(5))),
        search,
        studio,
        gpu,
        dir.join("state.json"),
    )
}

fn ask_job(text: &str) -> Job {
    Job {
        gid: "room".into(),
        trigger: "m1".into(),
        from: "alice".into(),
        text: text.into(),
        kind: JobKind::Answer { deepseek: false },
    }
}

fn file_names(c: &commons_core::client::Client, gid: &str) -> Vec<String> {
    c.messages(gid).into_iter().filter_map(|m| match m.body {
        MsgBody::File { name, .. } if !m.deleted => Some(name),
        _ => None,
    }).collect()
}

// ---------------------------------------------------------------- the whole way through the relay

#[test]
fn the_owner_gets_a_picture_posted_as_an_encrypted_attachment() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut oracle = Peer::new(&net, "Oracle");
    let (_cid, gen) = community_with(&net, &mut alice, &mut [&mut oracle]);

    let server = fake_studio(idle_state(), "PNGDATA");
    let dir = tempfile::tempdir().unwrap();
    let ollama = studio_ollama("picture", "a red barn at dawn, wide shot");
    let bot = make_bot(dir.path(), Config::default(), &ollama.url(), Arc::new(DeadSearch), studio_with(&server, dir.path()), Arc::new(NoGpu));

    alice.c.send_text(&gen, "@Oracle make me a picture of a red barn at dawn", None).unwrap();
    settle(&mut [&mut alice, &mut oracle]);
    let jobs = bot.poll(&mut oracle.c);
    assert_eq!(jobs.len(), 1, "one mention, one job");
    assert_eq!(jobs[0].from, alice.acct(), "the job knows who spoke");
    bot.answer(&mut oracle.c, &jobs[0]).expect("an answer");
    settle(&mut [&mut alice, &mut oracle]);

    // The picture arrived as an ordinary encrypted file message, named for the job, captioned with the prompt.
    let msgs = alice.c.messages(&gen);
    let file = msgs.iter().find_map(|m| match &m.body {
        MsgBody::File { name, mime, caption, .. } if !m.deleted => Some((name.clone(), mime.clone(), caption.clone())),
        _ => None,
    }).expect("alice sees the attachment");
    assert_eq!(file.0, "studio-1.png");
    assert_eq!(file.1, "image/png");
    assert_eq!(file.2, "a red barn at dawn, wide shot", "the prompt is the caption");
    assert!(texts(&alice, &gen).iter().any(|t| t == "Making it…"));

    // Exactly the four allowed routes, with Studio's token, and the picture body is what Studio served.
    assert_eq!(server.hits("/api/state"), 1);
    assert_eq!(server.hits("/api/jobs"), 1);
    assert_eq!(server.hits("/api/jobs/1"), 1);
    assert_eq!(server.hits("/img/1"), 1);
    let post = server.requests().into_iter().find(|r| r.path == "/api/jobs").unwrap();
    assert_eq!(post.header("x-homebase-token"), Some("test-token"), "the shared token is sent in its header");
    assert_eq!(post.json()["model"], "krea2");
    assert_eq!(post.json()["prompt"], "a red barn at dawn, wide shot");
    let file_msg = msgs.iter().find_map(|m| match &m.body {
        MsgBody::File { name, blob, key, .. } if name == "studio-1.png" => Some((blob.clone(), key.clone())),
        _ => None,
    }).unwrap();
    assert!(!file_msg.0.is_empty() && !file_msg.1.is_empty(), "the attachment is an encrypted blob, not plain bytes");
    assert!(!contains(&relay_bytes(&net), "PNGDATA"), "the picture never travels in the clear");
}

// ---------------------------------------------------------------- permissions and the admin commands

#[test]
fn a_non_owner_is_refused_until_an_admin_allows_them() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut bob = Peer::new(&net, "bob");
    let mut oracle = Peer::new(&net, "Oracle");
    let (_cid, gen) = community_with(&net, &mut alice, &mut [&mut bob, &mut oracle]);

    let server = fake_studio(idle_state(), "PNGDATA");
    let dir = tempfile::tempdir().unwrap();
    let ollama = studio_ollama("picture", "a red barn");
    let bot = make_bot(dir.path(), Config::default(), &ollama.url(), Arc::new(DeadSearch), studio_with(&server, dir.path()), Arc::new(NoGpu));

    // Bob is only a member: the bot refuses and says who can enable it. Studio is never contacted.
    bob.c.send_text(&gen, "@Oracle make me a picture of a red barn", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut oracle]);
    let jobs = bot.poll(&mut oracle.c);
    assert_eq!(jobs.len(), 1);
    bot.answer(&mut oracle.c, &jobs[0]).expect("an answer");
    settle(&mut [&mut alice, &mut bob, &mut oracle]);
    let refused = texts(&alice, &gen).into_iter().find(|t| t.contains("only for the community owner")).expect("a refusal");
    assert!(refused.contains("allow studio"), "the refusal says how to enable it: {refused}");
    assert_eq!(server.hits("/api/jobs"), 0, "nothing was sent to Studio");

    // Alice, the owner, allows Bob by name.
    alice.c.send_text(&gen, "@Oracle allow studio for bob", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut oracle]);
    let jobs = bot.poll(&mut oracle.c);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].kind, JobKind::SetStudio { on: true, person: Some(bob.acct()) });
    bot.answer(&mut oracle.c, &jobs[0]).expect("an answer");
    settle(&mut [&mut alice, &mut bob, &mut oracle]);
    assert!(has_text(&alice, &gen, "Studio is on for bob in this room."));

    // Now Bob's picture goes through.
    bob.c.send_text(&gen, "@Oracle make me a picture of a red barn", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut oracle]);
    let jobs = bot.poll(&mut oracle.c);
    assert_eq!(jobs.len(), 1);
    bot.answer(&mut oracle.c, &jobs[0]).expect("an answer");
    settle(&mut [&mut alice, &mut bob, &mut oracle]);
    assert_eq!(file_names(&bob.c, &gen), vec!["studio-1.png"]);
    assert_eq!(server.hits("/api/jobs"), 1);
}

// ---------------------------------------------------------------- the hard block, before anything is submitted

#[test]
fn the_hard_block_refuses_the_persons_own_words_before_any_job() {
    let dir = tempfile::tempdir().unwrap();
    let server = fake_studio(idle_state(), "PNGDATA");
    let ollama = studio_ollama("picture", "a harmless beach");
    let bot = make_bot(dir.path(), Config::default(), &ollama.url(), Arc::new(DeadSearch), studio_with(&server, dir.path()), Arc::new(NoGpu));

    let mut chat = FakeChat::new(Role::Owner);
    let sent = bot.answer(&mut chat, &ask_job("@Oracle draw a nude 14 year old girl")).expect("an answer");
    assert_eq!(sent.text, "Aight, done.", "the model's own follow-up is still posted");
    assert!(chat.sent.iter().any(|t| t == safety::REFUSAL), "the fixed refusal is posted");
    assert!(chat.files.is_empty());
    assert_eq!(server.hits("/api/state"), 0, "refused before Studio is even asked if it is free");
    assert_eq!(server.hits("/api/jobs"), 0);
}

#[test]
fn the_hard_block_refuses_a_prompt_the_model_wrote_before_any_job() {
    let dir = tempfile::tempdir().unwrap();
    let server = fake_studio(idle_state(), "PNGDATA");
    let ollama = studio_ollama("picture", "nude teen girl on a beach");
    let bot = make_bot(dir.path(), Config::default(), &ollama.url(), Arc::new(DeadSearch), studio_with(&server, dir.path()), Arc::new(NoGpu));

    let mut chat = FakeChat::new(Role::Owner);
    bot.answer(&mut chat, &ask_job("@Oracle make me a picture of the seaside")).expect("an answer");
    assert!(chat.sent.iter().any(|t| t == safety::REFUSAL), "the model-written prompt was refused");
    assert!(chat.files.is_empty());
    assert_eq!(server.hits("/api/jobs"), 0, "nothing reached Studio");
}

// ---------------------------------------------------------------- limits

#[test]
fn a_game_running_or_a_pause_stops_it_without_queueing() {
    for state in [json!({"game": true, "paused": ""}), json!({"game": false, "paused": "paused by alice"})] {
        let dir = tempfile::tempdir().unwrap();
        let server = fake_studio(state, "PNGDATA");
        let ollama = studio_ollama("picture", "a red barn");
        let bot = make_bot(dir.path(), Config::default(), &ollama.url(), Arc::new(DeadSearch), studio_with(&server, dir.path()), Arc::new(NoGpu));
        let mut chat = FakeChat::new(Role::Owner);
        bot.answer(&mut chat, &ask_job("@Oracle make me a picture of a red barn")).expect("an answer");
        assert!(chat.files.is_empty());
        assert!(chat.sent.iter().any(|t| t.contains("game is running") || t.contains("paused")), "the person is told: {:?}", chat.sent);
        assert_eq!(server.hits("/api/jobs"), 0, "nothing is queued for later");
    }
}

#[test]
fn the_daily_cap_counts_only_jobs_studio_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let server = fake_studio(idle_state(), "PNGDATA");
    let ollama = studio_ollama("picture", "a red barn");
    let mut cfg = Config::default();
    cfg.studio_daily_cap = 1;
    let bot = make_bot(dir.path(), cfg, &ollama.url(), Arc::new(DeadSearch), studio_with(&server, dir.path()), Arc::new(NoGpu));

    let mut chat = FakeChat::new(Role::Owner);
    bot.answer(&mut chat, &ask_job("@Oracle make me a picture of a red barn")).expect("an answer");
    assert_eq!(chat.files.len(), 1, "the first picture is made");
    bot.answer(&mut chat, &ask_job("@Oracle make me another picture")).expect("an answer");
    assert_eq!(chat.files.len(), 1, "the second is refused");
    assert!(chat.sent.iter().any(|t| t.contains("used all 1 Studio requests")), "{:?}", chat.sent);
    assert_eq!(server.hits("/api/jobs"), 1, "the capped request never reached Studio");
}

#[test]
fn video_is_owner_only_even_when_the_room_is_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let server = fake_studio(idle_state(), "MP4DATA");
    let ollama = studio_ollama("video", "a wave rolling in");
    // An admin turned Studio on for everyone in the room; video is still the owner's alone.
    std::fs::write(dir.path().join("state.json"), json!({
        "rooms": {"room": {"studio": {"everyone": true}}}
    }).to_string()).unwrap();
    let bot = make_bot(dir.path(), Config::default(), &ollama.url(), Arc::new(DeadSearch), studio_with(&server, dir.path()), Arc::new(NoGpu));

    let mut chat = FakeChat::new(Role::Member);
    bot.answer(&mut chat, &ask_job("@Oracle make me a video of a wave")).expect("an answer");
    assert!(chat.files.is_empty());
    assert!(chat.sent.iter().any(|t| t.contains("Video is only for the community owner")), "{:?}", chat.sent);
    assert_eq!(server.hits("/api/jobs"), 0);

    // The owner may.
    let mut owner = FakeChat::new(Role::Owner);
    bot.answer(&mut owner, &ask_job("@Oracle make me a video of a wave")).expect("an answer");
    assert_eq!(owner.files.len(), 1);
    assert_eq!(owner.files[0].0, "studio-1.mp4");
    assert_eq!(owner.files[0].1, "video/mp4");
    assert_eq!(server.hits("/api/jobs"), 1);
}

// ---------------------------------------------------------------- the two other kinds reach the right routes

#[test]
fn songs_and_sounds_use_the_audio_route_and_their_own_lengths() {
    let cases = [
        (json!({"kind": "song", "prompt": "warm reggae", "lyrics": "la la", "length": "60"}), "ace15xl", "60"),
        (json!({"kind": "sound", "prompt": "a door slam", "length": "6"}), "sfx", "6"),
    ];
    for (args, model, length) in cases {
        let dir = tempfile::tempdir().unwrap();
        let server = fake_studio(idle_state(), "MP3DATA");
        let ollama = studio_ollama_args(args);
        let bot = make_bot(dir.path(), Config::default(), &ollama.url(), Arc::new(DeadSearch), studio_with(&server, dir.path()), Arc::new(NoGpu));
        let mut chat = FakeChat::new(Role::Owner);
        bot.answer(&mut chat, &ask_job("@Oracle make me a sound")).expect("an answer");
        assert_eq!(chat.files.len(), 1);
        assert_eq!(chat.files[0].0, "studio-1.mp3");
        assert_eq!(chat.files[0].1, "audio/mpeg");
        let post = server.requests().into_iter().find(|r| r.path == "/api/jobs").unwrap();
        assert_eq!(post.json()["model"], model);
        assert_eq!(post.json()["length"], length);
        assert_eq!(server.hits("/aud/1"), 1);
    }
}
