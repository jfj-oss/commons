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

// ================================================================ Oracle's switch and the shared wall

use commons_bot::bot::{parse_wall_art, place_art, WallInfo};
use std::collections::HashSet;
use std::time::Instant;

const CAT: &str = " ##   ## \n ####### \n # o o # \n ###x### \n  -----  \n ~~~~~~~ ";
const ROCKET: &str = "```\n  ..  \n .##. \n.####.\n######\n  ||  \n  ||  \n```";

/// A fake Ollama that answers each chat call with the next of `replies` (the last one repeats). Nothing leaves the machine.
fn scripted_ollama(replies: &[&str]) -> FakeHttp {
    let replies: Vec<String> = replies.iter().map(|s| s.to_string()).collect();
    let n = Arc::new(AtomicUsize::new(0));
    FakeHttp::start(move |r| match r.path.as_str() {
        "/api/tags" => (200, json!({"models": [{"name": "qwen3.5:9b"}]}).to_string()),
        "/api/show" => (200, json!({"capabilities": ["completion", "tools"]}).to_string()),
        "/api/chat" => {
            let i = n.fetch_add(1, Ordering::SeqCst).min(replies.len() - 1);
            (200, json!({"model": "qwen3.5:9b", "message": {"role": "assistant", "content": replies[i]}, "done": true}).to_string())
        }
        "/api/generate" => (200, json!({"done": true}).to_string()),
        _ => (404, "{}".into()),
    })
}

/// A chat with a wall: records every stroke event with the time it was sent. After `lock_after` / `clear_after` strokes the owner
/// "locks" or "clears" the wall, as seen by the next check.
struct WallChat {
    inner: FakeChat,
    quiet: bool,
    info: WallInfo,
    strokes: Vec<(Instant, Vec<(u32, u32, u8, u8)>)>,
    lock_after: Option<usize>,
    clear_after: Option<usize>,
    refuse_first: usize,
}
impl WallChat {
    fn new() -> WallChat {
        WallChat { inner: chat_with_talk(), quiet: false, info: WallInfo::default(), strokes: vec![], lock_after: None, clear_after: None, refuse_first: 0 }
    }
    fn cells(&self) -> Vec<(u32, u32, u8, u8)> { self.strokes.iter().flat_map(|s| s.1.clone()).collect() }
}
impl Chat for WallChat {
    fn my_account(&self) -> String { self.inner.my_account() }
    fn history(&self, g: &str, l: usize) -> Vec<Line> { self.inner.history(g, l) }
    fn typing(&mut self, _: &str) {}
    fn send(&mut self, g: &str, t: &str, r: Option<String>) -> Result<String, String> { self.inner.send(g, t, r) }
    fn send_file(&mut self, g: &str, n: &str, m: &str, d: &[u8], c: &str) -> Result<String, String> { self.inner.send_file(g, n, m, d, c) }
    fn oracle_quiet(&self, _: &str) -> bool { self.quiet }
    fn wall_info(&self, _: &str) -> Option<WallInfo> { Some(self.info.clone()) }
    fn wall_draw(&mut self, _: &str, cells: &[(u32, u32, u8, u8)]) -> Result<bool, String> {
        if self.refuse_first > 0 { self.refuse_first -= 1; return Ok(false); }
        assert!(self.info.gen == 0 || !self.info.locked, "never drawn on a locked wall");
        self.strokes.push((Instant::now(), cells.to_vec()));
        let n = self.strokes.len();
        if self.lock_after == Some(n) { self.info.locked = true; }
        if self.clear_after == Some(n) { self.info.gen += 1; self.info.marks.clear(); }
        Ok(true)
    }
}
fn wall_job() -> Job { Job { gid: "g".into(), trigger: "t".into(), from: String::new(), text: String::new(), kind: JobKind::Wall } }
fn walled() -> Config { Config { wall_chance_pct: 100, wall_stroke_gap_ms: 0, wall_min_gap_secs: 0, ..Config::default() } }
fn wall_bot(dir: &Path, cfg: Config, ollama: &FakeHttp, gpu: Arc<dyn GpuCheck>) -> Bot {
    make_bot(dir, cfg, &ollama.url(), Arc::new(DeadSearch), Arc::new(DeadStudio), gpu)
}

// ---- the parser and the placement

#[test]
fn wall_art_is_parsed_strictly() {
    let a = parse_wall_art(ROCKET).expect("fenced art is fine");
    assert_eq!((a.width(), a.height()), (6, 6));
    assert!(parse_wall_art(CAT).is_some());
    assert!(parse_wall_art("Sure! Here is a cat for you").is_none(), "a sentence is not art");
    assert!(parse_wall_art("  ..  \n .##. \n.#a##.\n######\n  ||  ").is_none(), "a letter outside the glyph set");
    assert!(parse_wall_art("  ..  \n .##. \n.#\t##.\n######\n  ||  ").is_none(), "a tab");
    assert!(parse_wall_art("  ..  \n .##. \n.####.@\n######\n  ||  ").is_none(), "an @");
    assert!(parse_wall_art(&format!("{0}\n{0}\n{0}", "#".repeat(41))).is_none(), "too wide");
    assert!(parse_wall_art(&vec!["#####"; 17].join("\n")).is_none(), "too tall");
    assert!(parse_wall_art(&vec!["#####"; 16].join("\n")).is_some(), "16 rows fit");
    assert!(parse_wall_art(&format!("{0}\n{0}\n{0}", "#".repeat(40))).is_some(), "40 columns fit");
    assert!(parse_wall_art("#\n\n.").is_none(), "too few marks");
    assert!(parse_wall_art("").is_none());
}

#[test]
fn placement_never_touches_existing_marks_and_gives_up_when_it_must() {
    let art = parse_wall_art(ROCKET).unwrap();
    let cols = commons_core::wall::WALL_COLS;
    // Everything but a pocket in the right of the wall is marked.
    let mut marks: HashSet<u32> = (0..commons_core::wall::WALL_CELLS).collect();
    for y in 20..30 { for x in 90..100 { marks.remove(&(y * cols + x)); } }
    let (ox, oy) = place_art(&art, &marks).expect("the pocket is big enough");
    assert!((90..=94).contains(&ox) && (20..=24).contains(&oy), "it went into the pocket, got {ox},{oy}");
    for s in art.strokes(ox, oy) { for (x, y, ..) in s { assert!(!marks.contains(&(y * cols + x))); } }
    // A pocket too small for it: nothing is drawn rather than drawing over others.
    for y in 20..30 { for x in 90..100 { marks.insert(y * cols + x); } }
    for y in 20..24 { for x in 90..94 { marks.remove(&(y * cols + x)); } }
    assert!(place_art(&art, &marks).is_none());
    // Spaces inside the piece are not marks: a thin mark may sit in the hollow of a bigger one.
    let ring = parse_wall_art("#####\n#   #\n#   #\n#####").unwrap();
    let mut m2: HashSet<u32> = (0..commons_core::wall::WALL_CELLS).collect();
    for y in 10..14 { for x in 10..15 { m2.remove(&(y * cols + x)); } }
    m2.insert(11 * cols + 12);
    let (rx, ry) = place_art(&ring, &m2).expect("the hollow of a ring is not a mark of the ring");
    for s in ring.strokes(rx, ry) { for (x, y, ..) in s { assert!(!m2.contains(&(y * cols + x))); } }
}

// ---- the model's art on the wall

#[test]
fn oracle_draws_the_art_stroke_by_stroke_within_the_budget() {
    let ollama = scripted_ollama(&[CAT]);
    let dir = tempfile::tempdir().unwrap();
    // The real default pacing: no more than two events a second.
    let bot = wall_bot(dir.path(), Config { wall_stroke_gap_ms: Config::default().wall_stroke_gap_ms, ..walled() }, &ollama, Arc::new(NoGpu));
    let mut chat = WallChat::new();
    bot.answer(&mut chat, &wall_job());
    let art = parse_wall_art(CAT).unwrap();
    assert!(chat.strokes.len() >= 2, "it appears in more than one go: {}", chat.strokes.len());
    for (_, s) in &chat.strokes {
        assert!(!s.is_empty() && s.len() <= commons_core::wall::WALL_BATCH, "an event has 1 to 200 cells");
        assert!(s.iter().all(|&(x, y, g, c)| x < 120 && y < 54 && g >= 1 && (g as usize) < commons_core::wall::WALL_GLYPHS.len() && c == 0), "only glyphs, never an erase");
    }
    for w in chat.strokes.windows(2) {
        assert!(w[1].0.duration_since(w[0].0) >= Duration::from_millis(450), "at most about two events a second");
    }
    let drawn: usize = chat.cells().len();
    let want: usize = art.strokes(0, 0).iter().map(|s| s.len()).sum();
    assert_eq!(drawn, want, "every mark of the art, nothing else");
    assert!(chat.inner.sent.is_empty() && chat.inner.files.is_empty(), "it says nothing about it");
    assert_eq!(ollama.hits("/api/chat"), 1);
    // The prompt that went out carries the safety words and the glyph set.
    let body = ollama.requests().into_iter().find(|r| r.path == "/api/chat").unwrap().json().to_string();
    assert!(body.contains("minors") && body.contains("real person"), "{body}");
    assert!(body.contains("# * + . o = ~ / | - x"));
}

#[test]
fn wall_art_avoids_marks_that_are_already_there_and_is_not_drawn_when_there_is_no_room() {
    let ollama = scripted_ollama(&[ROCKET]);
    let dir = tempfile::tempdir().unwrap();
    let bot = wall_bot(dir.path(), walled(), &ollama, Arc::new(NoGpu));
    let mut chat = WallChat::new();
    // Marks over the whole left half and the top.
    for y in 0..54u32 { for x in 0..60u32 { chat.info.marks.insert(y * 120 + x); } }
    bot.answer(&mut chat, &wall_job());
    assert!(!chat.cells().is_empty());
    assert!(chat.cells().iter().all(|&(x, y, ..)| !chat.info.marks.contains(&(y * 120 + x))), "no overlap");
    // A wall that is full: nothing is drawn.
    let dir2 = tempfile::tempdir().unwrap();
    let bot = wall_bot(dir2.path(), walled(), &ollama, Arc::new(NoGpu));
    let mut full = WallChat::new();
    full.info.marks = (0..commons_core::wall::WALL_CELLS).collect();
    bot.answer(&mut full, &wall_job());
    assert!(full.strokes.is_empty());
}

#[test]
fn malformed_art_draws_nothing_and_one_retry_is_allowed() {
    for bad in ["Sure thing, here is a cat!", "####################################################\n#\n#", "..\n.a.\n.."] {
        let ollama = scripted_ollama(&[bad]);
        let dir = tempfile::tempdir().unwrap();
        let bot = wall_bot(dir.path(), walled(), &ollama, Arc::new(NoGpu));
        let mut chat = WallChat::new();
        bot.answer(&mut chat, &wall_job());
        assert!(chat.strokes.is_empty(), "nothing usable, nothing drawn: {bad:?}");
        assert_eq!(ollama.hits("/api/chat"), 2, "it asked once more, and no more");
    }
    let ollama = scripted_ollama(&["no idea, sorry", ROCKET]);
    let dir = tempfile::tempdir().unwrap();
    let bot = wall_bot(dir.path(), walled(), &ollama, Arc::new(NoGpu));
    let mut chat = WallChat::new();
    bot.answer(&mut chat, &wall_job());
    assert!(!chat.strokes.is_empty(), "the second try was good");
    assert_eq!(ollama.hits("/api/chat"), 2);
}

#[test]
fn the_hard_block_covers_wall_art_and_what_comes_with_it() {
    let ollama = scripted_ollama(&[&format!("a nude teen\n{ROCKET}")]);
    let dir = tempfile::tempdir().unwrap();
    let bot = wall_bot(dir.path(), walled(), &ollama, Arc::new(NoGpu));
    let mut chat = WallChat::new();
    bot.answer(&mut chat, &wall_job());
    assert!(chat.strokes.is_empty(), "blocked output is never drawn");
    assert_eq!(ollama.hits("/api/chat"), 1, "and it is not asked again");
    assert!(chat.inner.sent.is_empty());
    // The wording reaches the model only through our own fixed prompt, which the same check passes.
    assert!(safety::reason("Draw a small ASCII picture of a cat.").is_none());
}

#[test]
fn a_locked_wall_is_left_alone_and_a_lock_or_clear_mid_drawing_stops_it() {
    let ollama = scripted_ollama(&[CAT]);
    let dir = tempfile::tempdir().unwrap();
    let bot = wall_bot(dir.path(), walled(), &ollama, Arc::new(NoGpu));
    let mut locked = WallChat::new();
    locked.info.locked = true;
    bot.answer(&mut locked, &wall_job());
    assert!(locked.strokes.is_empty());
    assert_eq!(ollama.hits("/api/chat"), 0, "no model call for a wall it may not draw on");

    let total = parse_wall_art(CAT).unwrap().strokes(0, 0).len();
    assert!(total >= 3);
    let mut mid = WallChat::new();
    mid.lock_after = Some(1);
    let dir2 = tempfile::tempdir().unwrap();
    wall_bot(dir2.path(), walled(), &ollama, Arc::new(NoGpu)).answer(&mut mid, &wall_job());
    assert_eq!(mid.strokes.len(), 1, "locked after the first event: it stops");

    let mut cleared = WallChat::new();
    cleared.clear_after = Some(1);
    let dir3 = tempfile::tempdir().unwrap();
    wall_bot(dir3.path(), walled(), &ollama, Arc::new(NoGpu)).answer(&mut cleared, &wall_job());
    assert_eq!(cleared.strokes.len(), 1, "cleared after the first event: it does not bring the picture back");
}

#[test]
fn the_wall_switch_quiet_cap_gpu_and_a_busy_core_are_respected() {
    let ollama = scripted_ollama(&[CAT]);
    // Quiet: no model call, nothing drawn.
    let dir = tempfile::tempdir().unwrap();
    let mut q = WallChat::new();
    q.quiet = true;
    wall_bot(dir.path(), walled(), &ollama, Arc::new(NoGpu)).answer(&mut q, &wall_job());
    assert!(q.strokes.is_empty());
    // Quiet switched on while the model is thinking is noticed before the first stroke too.
    // The GPU belongs to Studio or a game: the model is not even asked.
    let dir = tempfile::tempdir().unwrap();
    let mut g = WallChat::new();
    wall_bot(dir.path(), walled(), &ollama, Arc::new(BusyGpu)).answer(&mut g, &wall_job());
    assert!(g.strokes.is_empty());
    assert_eq!(ollama.hits("/api/chat"), 0);
    // The daily cap per room: with 1 allowed, the second job draws nothing.
    let dir = tempfile::tempdir().unwrap();
    let bot = wall_bot(dir.path(), Config { wall_daily_max: 1, ..walled() }, &ollama, Arc::new(NoGpu));
    let mut c1 = WallChat::new();
    bot.answer(&mut c1, &wall_job());
    let mut c2 = WallChat::new();
    bot.answer(&mut c2, &wall_job());
    assert!(!c1.strokes.is_empty());
    assert!(c2.strokes.is_empty(), "the cap holds");
    // A core that says "later" is asked again, a few times, then it gives up quietly.
    let dir = tempfile::tempdir().unwrap();
    let mut busy = WallChat::new();
    busy.refuse_first = 2;
    wall_bot(dir.path(), Config { wall_stroke_gap_ms: 0, ..walled() }, &ollama, Arc::new(NoGpu)).answer(&mut busy, &wall_job());
    assert_eq!(busy.cells().len(), parse_wall_art(CAT).unwrap().strokes(0, 0).iter().map(|s| s.len()).sum::<usize>());
    let dir = tempfile::tempdir().unwrap();
    let mut stuck = WallChat::new();
    stuck.refuse_first = 100;
    wall_bot(dir.path(), walled(), &ollama, Arc::new(NoGpu)).answer(&mut stuck, &wall_job());
    assert!(stuck.strokes.is_empty());
}

#[test]
fn a_mention_is_still_answered_in_a_quiet_room() {
    let ollama = fake_ollama(&["qwen3.5:9b"], "mi deh yah");
    let dir = tempfile::tempdir().unwrap();
    let bot = make_bot(dir.path(), Config::default(), &ollama.url(), Arc::new(DeadSearch), Arc::new(DeadStudio), Arc::new(NoGpu));
    let mut chat = WallChat::new();
    chat.quiet = true;
    chat.inner.lines = vec![Line { mine: false, name: "alice".into(), text: "@Oracle hello?".into() }];
    bot.answer(&mut chat, &ask_job("@Oracle hello?")).expect("answered");
    assert_eq!(chat.inner.sent, vec!["mi deh yah".to_string()]);
}

#[test]
fn quiet_stops_chiming_and_surprise_art() {
    let studio = fake_studio(idle_state(), "PNG");
    let ollama = studio_ollama("picture", "a neon goat");
    let dir = tempfile::tempdir().unwrap();
    let bot = make_bot(dir.path(), arty(), &ollama.url(), Arc::new(DeadSearch), studio_with(&studio, dir.path()), Arc::new(NoGpu));
    let mut chat = WallChat::new();
    chat.quiet = true;
    assert!(bot.answer(&mut chat, &chime_job()).is_none());
    assert!(chat.inner.sent.is_empty() && chat.inner.files.is_empty());
    assert_eq!(ollama.hits("/api/chat"), 0, "the model is not even asked");
    assert_eq!(posts_to_studio(&studio), 0);
    chat.quiet = false;
    assert!(bot.answer(&mut chat, &chime_job()).is_some(), "and it follows the switch back on");
}

// ---- through the real core: the owner's signed flag

fn quiet_flag(p: &Peer, gen: &str) -> bool {
    let snap = commons_core::api::debug_snapshot(&p.c);
    snap["communities"][0]["style"]["looks"][gen]["oracle_quiet"].as_bool().unwrap_or(false)
}

#[test]
fn only_the_owner_can_set_the_flag_and_the_bot_follows_it_live() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut oracle = Peer::new(&net, "Oracle");
    let (cid, gen) = community_with(&net, &mut alice, &mut [&mut oracle]);
    let ollama = scripted_ollama(&["pasta a fi pickney"]);
    let dir = tempfile::tempdir().unwrap();
    let bot = make_bot(dir.path(), Config { wall_chance_pct: 100, wall_min_gap_secs: 0, wall_stroke_gap_ms: 0, ..chatty() }, &ollama.url(), Arc::new(DeadSearch), Arc::new(DeadStudio), Arc::new(NoGpu));
    let say = |who: &mut Peer, other: &mut Peer, t: &str| { who.c.send_text(&gen, t, None).unwrap(); settle(&mut [who, other]); };
    let _ = bot.poll(&mut oracle.c);

    // A member cannot set it, by either route, and nothing is signed.
    assert!(oracle.c.set_oracle_quiet(&cid, &gen, true).is_err());
    assert!(oracle.c.set_look(&cid, Some(&gen), Some(commons_core::envelope::Look { oracle_quiet: Some(true), ..Default::default() })).is_err());
    settle(&mut [&mut alice, &mut oracle]);
    assert!(!quiet_flag(&alice, &gen) && !quiet_flag(&oracle, &gen));

    // Without the flag it talks and draws on a lively room.
    say(&mut alice, &mut oracle, "one");
    let kinds: Vec<JobKind> = bot.poll(&mut oracle.c).into_iter().map(|j| j.kind).collect();
    assert!(kinds.contains(&JobKind::Chime) && kinds.contains(&JobKind::Wall), "{kinds:?}");

    // The owner switches it off. The bot sees it through its own synced state, no restart.
    alice.c.set_oracle_quiet(&cid, &gen, true).unwrap();
    settle(&mut [&mut alice, &mut oracle]);
    assert!(quiet_flag(&alice, &gen) && quiet_flag(&oracle, &gen), "in the snapshot beside looks");
    assert!(alice.c.look_of(&cid, Some(&gen)).unwrap().oracle_quiet == Some(true));
    // Patterns set afterwards through set_look keep the flag unless it is named.
    alice.c.set_look(&cid, Some(&gen), Some(commons_core::envelope::Look { chat: Some(3), oracle_quiet: alice.c.look_of(&cid, Some(&gen)).and_then(|l| l.oracle_quiet), ..Default::default() })).unwrap();
    settle(&mut [&mut alice, &mut oracle]);
    say(&mut alice, &mut oracle, "two");
    say(&mut alice, &mut oracle, "three");
    assert!(bot.poll(&mut oracle.c).is_empty(), "no chime, no art, no drawing");

    // The owner's flag beats "@Oracle chime in". A mention is still answered.
    say(&mut alice, &mut oracle, "@Oracle chime in");
    let jobs = bot.poll(&mut oracle.c);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].kind, JobKind::SetChime { on: true });
    let sent = bot.answer(&mut oracle.c, &jobs[0]).expect("it explains");
    assert!(sent.text.contains("owner"), "{}", sent.text);
    say(&mut alice, &mut oracle, "chatter");
    assert!(bot.poll(&mut oracle.c).is_empty(), "still quiet after the chat command");
    say(&mut alice, &mut oracle, "@Oracle hello there");
    let jobs = bot.poll(&mut oracle.c);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].kind, JobKind::Answer { deepseek: false });
    assert!(bot.answer(&mut oracle.c, &jobs[0]).is_some(), "someone asked");
    // "@Oracle quiet" still works for what it did.
    say(&mut alice, &mut oracle, "@Oracle quiet");
    assert_eq!(bot.poll(&mut oracle.c)[0].kind, JobKind::SetChime { on: false });

    // Off again: the owner's switch no longer holds it back (the chat words still do).
    alice.c.set_oracle_quiet(&cid, &gen, false).unwrap();
    settle(&mut [&mut alice, &mut oracle]);
    assert!(!quiet_flag(&alice, &gen));
    say(&mut alice, &mut oracle, "@Oracle chime in");
    assert_eq!(bot.poll(&mut oracle.c)[0].kind, JobKind::SetChime { on: true });
    say(&mut alice, &mut oracle, "talking");
    let kinds: Vec<JobKind> = bot.poll(&mut oracle.c).into_iter().map(|j| j.kind).collect();
    assert!(kinds.contains(&JobKind::Chime), "{kinds:?}");
}

#[test]
fn the_bots_drawing_reaches_everyone_through_the_core_and_a_lock_or_clear_is_obeyed() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut oracle = Peer::new(&net, "Oracle");
    let (cid, gen) = community_with(&net, &mut alice, &mut [&mut oracle]);
    let ollama = scripted_ollama(&[ROCKET]);
    let dir = tempfile::tempdir().unwrap();
    let bot = wall_bot(dir.path(), walled(), &ollama, Arc::new(NoGpu));
    let job = Job { gid: gen.clone(), ..wall_job() };

    // Locked by the owner: the core would refuse anyway, and the bot does not even ask the model.
    alice.c.set_wall(&cid, &gen, Some(true), false).unwrap();
    settle(&mut [&mut alice, &mut oracle]);
    bot.answer(&mut oracle.c, &job);
    assert_eq!(ollama.hits("/api/chat"), 0);
    assert!(oracle.c.wall_view(&gen).cells.is_empty());

    alice.c.set_wall(&cid, &gen, Some(false), false).unwrap();
    settle(&mut [&mut alice, &mut oracle]);
    bot.answer(&mut oracle.c, &job);
    for _ in 0..4 { settle(&mut [&mut alice, &mut oracle]); }
    let mine = oracle.c.wall_view(&gen).cells.len();
    assert!(mine >= 8, "drawn on its own wall: {mine}");
    assert_eq!(alice.c.wall_view(&gen).cells.len(), mine, "and everyone sees the same");
    assert!(alice.c.wall_view(&gen).cells.iter().all(|c| c.3 == oracle.acct()), "marked by nothing but its own account");

    // The owner clears it: the new generation is empty, and a later drawing goes on the new one.
    alice.c.set_wall(&cid, &gen, None, true).unwrap();
    settle(&mut [&mut alice, &mut oracle]);
    assert!(alice.c.wall_view(&gen).cells.is_empty());
}
