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
fn ten_songs_in_one_request_are_made_one_after_another() {
    let dir = tempfile::tempdir().unwrap();
    let server = fake_studio(idle_state(), "MP3DATA");
    let ollama = studio_ollama_args(json!({"kind": "song", "prompt": "warm reggae", "lyrics": "la la"}));
    let bot = make_bot(dir.path(), Config::default(), &ollama.url(), Arc::new(DeadSearch), studio_with(&server, dir.path()), Arc::new(NoGpu));
    let mut chat = FakeChat::new(Role::Owner);
    bot.answer(&mut chat, &ask_job("@Oracle make 10 songs about my cat")).expect("an answer");
    assert_eq!(server.hits("/api/jobs"), 10, "one job per song");
    assert_eq!(chat.files.len(), 10, "every song is posted");
}

#[test]
fn asking_for_more_than_ten_songs_makes_ten() {
    let dir = tempfile::tempdir().unwrap();
    let server = fake_studio(idle_state(), "MP3DATA");
    let ollama = studio_ollama_args(json!({"kind": "song", "prompt": "warm reggae", "lyrics": "la la", "count": 40}));
    let bot = make_bot(dir.path(), Config::default(), &ollama.url(), Arc::new(DeadSearch), studio_with(&server, dir.path()), Arc::new(NoGpu));
    let mut chat = FakeChat::new(Role::Owner);
    bot.answer(&mut chat, &ask_job("@Oracle make me some songs")).expect("an answer");
    assert_eq!(server.hits("/api/jobs"), 10);
}

#[test]
fn songs_and_sounds_use_the_audio_route_and_their_own_lengths() {
    let cases = [
        (json!({"kind": "song", "prompt": "warm reggae", "lyrics": "la la", "length": "60"}), "yue2", "60"),
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

// ---------------------------------------------------------------- joining in without being asked

fn chatty() -> Config {
    Config { chime_chance_pct: 100, chime_min_gap_secs: 0, chime_min_messages: 1, chime_delay_secs: (0, 0), art_min_gap_secs: 0, ..Config::default() }
}
fn chime_job() -> Job { Job { gid: "g".into(), trigger: "t".into(), from: String::new(), text: String::new(), kind: JobKind::Chime } }
fn chat_with_talk() -> FakeChat {
    let mut chat = FakeChat::new(Role::Member);
    chat.lines = vec![
        Line { mine: false, name: "alice".into(), text: "best pasta shape?".into() },
        Line { mine: false, name: "bob".into(), text: "penne obviously".into() },
    ];
    chat
}
struct BusyGpu;
impl GpuCheck for BusyGpu { fn blocked(&self) -> Option<commons_bot::gpu::Pause> { Some(commons_bot::gpu::Pause::Studio) } }

#[test]
fn joining_in_posts_a_short_remark_that_pings_nobody() {
    let ollama = fake_ollama(&["qwen3.5:9b"], "Oracle: @alice yuh mad, penne a fi fool dem");
    let dir = tempfile::tempdir().unwrap();
    let bot = make_bot(dir.path(), chatty(), &ollama.url(), Arc::new(DeadSearch), Arc::new(DeadStudio), Arc::new(NoGpu));
    let mut chat = chat_with_talk();
    bot.answer(&mut chat, &chime_job()).expect("a remark");
    assert_eq!(chat.sent, vec!["alice yuh mad, penne a fi fool dem".to_string()]);
    let body = ollama.requests().into_iter().find(|r| r.path == "/api/chat").unwrap().json();
    assert!(body.get("tools").is_none_or(|t| t.as_array().is_none_or(|a| a.is_empty())), "no tools when it is only talking");
}

#[test]
fn it_can_decide_it_has_nothing_to_say() {
    let ollama = fake_ollama(&["qwen3.5:9b"], "PASS.");
    let dir = tempfile::tempdir().unwrap();
    let bot = make_bot(dir.path(), chatty(), &ollama.url(), Arc::new(DeadSearch), Arc::new(DeadStudio), Arc::new(NoGpu));
    let mut chat = chat_with_talk();
    assert!(bot.answer(&mut chat, &chime_job()).is_none());
    assert!(chat.sent.is_empty());
}

#[test]
fn it_never_loads_the_model_unasked_while_the_gpu_is_taken() {
    let ollama = fake_ollama(&["qwen3.5:9b"], "words");
    let dir = tempfile::tempdir().unwrap();
    let bot = make_bot(dir.path(), chatty(), &ollama.url(), Arc::new(DeadSearch), Arc::new(DeadStudio), Arc::new(BusyGpu));
    let mut chat = chat_with_talk();
    assert!(bot.answer(&mut chat, &chime_job()).is_none());
    assert_eq!(ollama.hits("/api/chat"), 0);
    assert!(chat.sent.is_empty(), "no notice either: nobody asked");
}

fn arty() -> Config { Config { art_chance_pct: 100, ..chatty() } }
fn posts_to_studio(s: &FakeHttp) -> usize { s.requests().iter().filter(|r| r.method == "POST" && r.path.starts_with("/api/jobs")).count() }

#[test]
fn it_makes_something_for_fun_and_posts_it() {
    let studio = fake_studio(idle_state(), "PNGDATA");
    let ollama = studio_ollama("picture", "a neon goat riding a bus");
    let dir = tempfile::tempdir().unwrap();
    let bot = make_bot(dir.path(), arty(), &ollama.url(), Arc::new(DeadSearch), studio_with(&studio, dir.path()), Arc::new(NoGpu));
    let mut chat = chat_with_talk();
    bot.answer(&mut chat, &chime_job()).expect("something made");
    assert_eq!(chat.files.len(), 1);
    assert_eq!(chat.files[0].3, "a neon goat riding a bus");
    assert!(chat.sent.is_empty(), "no \"Making it…\" line: it is a surprise");
}

#[test]
fn a_song_made_for_fun_is_always_short() {
    let studio = fake_studio(idle_state(), "AUDIO");
    let ollama = studio_ollama_args(json!({"kind": "song", "prompt": "warm reggae about pasta", "lyrics": "la la", "length": "180"}));
    let dir = tempfile::tempdir().unwrap();
    let bot = make_bot(dir.path(), arty(), &ollama.url(), Arc::new(DeadSearch), studio_with(&studio, dir.path()), Arc::new(NoGpu));
    bot.answer(&mut chat_with_talk(), &chime_job()).expect("a song");
    let sub = studio.requests().into_iter().find(|r| r.method == "POST" && r.path.starts_with("/api/jobs")).unwrap().json();
    assert_eq!(sub["length"], "30");
}

#[test]
fn video_is_never_made_for_fun_and_it_just_talks_instead() {
    let studio = fake_studio(idle_state(), "VID");
    let ollama = studio_ollama("video", "a goat");
    let dir = tempfile::tempdir().unwrap();
    let bot = make_bot(dir.path(), arty(), &ollama.url(), Arc::new(DeadSearch), studio_with(&studio, dir.path()), Arc::new(NoGpu));
    let mut chat = chat_with_talk();
    bot.answer(&mut chat, &chime_job());
    assert_eq!(posts_to_studio(&studio), 0);
    assert!(chat.files.is_empty());
    assert_eq!(chat.sent, vec!["Aight, done.".to_string()]);
}

#[test]
fn the_hard_block_covers_what_it_makes_for_fun() {
    let studio = fake_studio(idle_state(), "PNG");
    let ollama = studio_ollama("picture", "a nude teen");
    let dir = tempfile::tempdir().unwrap();
    let bot = make_bot(dir.path(), arty(), &ollama.url(), Arc::new(DeadSearch), studio_with(&studio, dir.path()), Arc::new(NoGpu));
    let mut chat = chat_with_talk();
    bot.answer(&mut chat, &chime_job());
    assert_eq!(posts_to_studio(&studio), 0, "Studio is never contacted");
    assert!(chat.files.is_empty());
    assert!(!chat.sent.iter().any(|t| t == safety::REFUSAL), "no refusal for something nobody asked for");
}

#[test]
fn a_game_means_no_art_for_fun() {
    let studio = fake_studio(json!({"game": true, "paused": ""}), "PNG");
    let ollama = fake_ollama(&["qwen3.5:9b"], "words instead");
    let dir = tempfile::tempdir().unwrap();
    let bot = make_bot(dir.path(), arty(), &ollama.url(), Arc::new(DeadSearch), studio_with(&studio, dir.path()), Arc::new(NoGpu));
    let mut chat = chat_with_talk();
    bot.answer(&mut chat, &chime_job());
    assert_eq!(posts_to_studio(&studio), 0);
    assert!(chat.files.is_empty());
}

#[test]
fn art_for_fun_stops_at_the_daily_cap() {
    let studio = fake_studio(idle_state(), "PNG");
    let ollama = studio_ollama("picture", "a neon goat");
    let dir = tempfile::tempdir().unwrap();
    let bot = make_bot(dir.path(), Config { art_daily_max: 1, ..arty() }, &ollama.url(), Arc::new(DeadSearch), studio_with(&studio, dir.path()), Arc::new(NoGpu));
    let mut chat = chat_with_talk();
    bot.answer(&mut chat, &chime_job());
    bot.answer(&mut chat, &chime_job());
    assert_eq!(chat.files.len(), 1);
    assert_eq!(posts_to_studio(&studio), 1);
    assert_eq!(chat.sent.len(), 1, "after the cap it only talks");
}

#[test]
fn joining_in_needs_a_live_conversation_and_respects_the_switches() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut oracle = Peer::new(&net, "Oracle");
    let (_cid, gen) = community_with(&net, &mut alice, &mut [&mut oracle]);
    let ollama = fake_ollama(&["qwen3.5:9b"], "pasta a fi pickney");
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config { chime_min_messages: 3, chime_min_gap_secs: 1000, ..chatty() };
    let bot = make_bot(dir.path(), cfg, &ollama.url(), Arc::new(DeadSearch), Arc::new(DeadStudio), Arc::new(NoGpu));
    assert!(bot.poll(&mut oracle.c).is_empty());

    let say = |who: &mut Peer, other: &mut Peer, t: &str| { who.c.send_text(&gen, t, None).unwrap(); settle(&mut [who, other]); };
    say(&mut alice, &mut oracle, "one");
    say(&mut alice, &mut oracle, "two");
    assert!(bot.poll(&mut oracle.c).is_empty(), "two messages is not a conversation yet");
    say(&mut alice, &mut oracle, "three");
    let jobs = bot.poll(&mut oracle.c);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].kind, JobKind::Chime);
    let sent = bot.answer(&mut oracle.c, &jobs[0]).expect("a remark");
    settle(&mut [&mut alice, &mut oracle]);
    assert!(has_text(&alice, &gen, &sent.text), "everyone sees it");
    let last = alice.c.st.chans[&gen].msgs.iter().rev().find(|m| matches!(&m.body, MsgBody::Text(t) if *t == sent.text)).unwrap();
    assert!(last.reply.is_none(), "it replies to nobody");

    for t in ["four", "five", "six"] { say(&mut alice, &mut oracle, t); }
    assert!(bot.poll(&mut oracle.c).is_empty(), "the gap holds it back");

    // A second bot with no gap, to exercise the switches. A mention wins over joining in.
    let dir2 = tempfile::tempdir().unwrap();
    let bot = make_bot(dir2.path(), chatty(), &ollama.url(), Arc::new(DeadSearch), Arc::new(DeadStudio), Arc::new(NoGpu));
    let _ = bot.poll(&mut oracle.c);
    say(&mut alice, &mut oracle, "@Oracle hello");
    let jobs = bot.poll(&mut oracle.c);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].kind, JobKind::Answer { deepseek: false });

    say(&mut alice, &mut oracle, "@Oracle quiet");
    let jobs = bot.poll(&mut oracle.c);
    assert_eq!(jobs[0].kind, JobKind::SetChime { on: false });
    say(&mut alice, &mut oracle, "chatter");
    assert!(bot.poll(&mut oracle.c).is_empty(), "quiet means quiet");
    say(&mut alice, &mut oracle, "@Oracle chime in");
    let jobs = bot.poll(&mut oracle.c);
    assert_eq!(jobs[0].kind, JobKind::SetChime { on: true });
    say(&mut alice, &mut oracle, "chatter again");
    assert_eq!(bot.poll(&mut oracle.c)[0].kind, JobKind::Chime);

    // In a DeepSeek room the words would leave the computer, and nobody asked for that.
    say(&mut alice, &mut oracle, "@Oracle use deepseek");
    let _ = bot.poll(&mut oracle.c);
    say(&mut alice, &mut oracle, "more chatter");
    assert!(bot.poll(&mut oracle.c).is_empty());
}
