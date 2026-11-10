//! Oracle's daily profile picture: once a day across restarts, the GPU gate, the safety rules, Studio failing, and what
//! reaches the core (a square picture under the core's size limit). Fake model, fake Studio, fake chat: nothing real.
mod common;
use common::*;
use commons_bot::bot::{AvatarOutcome, Bot, Chat, Config, Line};
use commons_bot::gpu::{GpuCheck, Pause};
use commons_bot::llm::{DeepSeek, Ollama};
use commons_bot::studio::{JobState, Kind, Request, State, StudioApi};
use commons_core::envelope::MAX_AVATAR;
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const GOOD: &str = "LOOK: a cursed mango prophet with three calm eyes and a tin-foil crown, orange fog behind\nLINE: Wah gwaan bredda, mi new face a di truth, Babylon nuh waan yuh see it";

struct FakeStudio {
    game: AtomicBool,
    job_fails: AtomicBool,
    garbage: AtomicBool,
    submits: Mutex<Vec<Request>>,
    png: Vec<u8>,
}
impl FakeStudio {
    fn new() -> Arc<FakeStudio> {
        Arc::new(FakeStudio { game: AtomicBool::new(false), job_fails: AtomicBool::new(false), garbage: AtomicBool::new(false), submits: Mutex::new(vec![]), png: big_png() })
    }
    fn submitted(&self) -> usize { self.submits.lock().unwrap().len() }
}
impl StudioApi for FakeStudio {
    fn state(&self) -> Result<State, String> { Ok(State { game: self.game.load(Ordering::SeqCst), paused: String::new() }) }
    fn submit(&self, req: &Request) -> Result<u64, String> { self.submits.lock().unwrap().push(req.clone()); Ok(1) }
    fn status(&self, _id: u64) -> Result<JobState, String> {
        Ok(if self.job_fails.load(Ordering::SeqCst) { JobState::Failed("out of memory".into()) } else { JobState::Done })
    }
    fn download(&self, _kind: Kind, _id: u64) -> Result<Vec<u8>, String> {
        Ok(if self.garbage.load(Ordering::SeqCst) { b"not a picture".to_vec() } else { self.png.clone() })
    }
}

/// A 1024x768 noisy PNG, like a real render: it cannot fit the limit at full size.
fn big_png() -> Vec<u8> {
    let (w, h) = (1024u32, 768u32);
    let mut px = Vec::with_capacity((w * h * 3) as usize);
    let mut s = 99u32;
    for y in 0..h { for x in 0..w {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        px.extend_from_slice(&[((x / 4) as u8).wrapping_add((s >> 28) as u8), (y / 3) as u8, ((s >> 24) as u8) / 3]);
    } }
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(png::ColorType::Rgb);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().unwrap().write_image_data(&px).unwrap();
    }
    out
}

struct Gpu(AtomicBool);
impl GpuCheck for Gpu {
    fn blocked(&self) -> Option<Pause> { if self.0.load(Ordering::SeqCst) { Some(Pause::Studio) } else { None } }
}

struct FakeChat {
    rooms: Vec<String>,
    quiet: HashSet<String>,
    avatars: Vec<(String, usize)>,
    sent: Vec<(String, String)>,
}
impl FakeChat {
    fn new() -> FakeChat { FakeChat { rooms: vec!["a".into(), "b".into(), "hush".into()], quiet: ["hush".to_string()].into(), avatars: vec![], sent: vec![] } }
}
impl Chat for FakeChat {
    fn my_account(&self) -> String { "bot".into() }
    fn history(&self, _gid: &str, _limit: usize) -> Vec<Line> { vec![] }
    fn typing(&mut self, _gid: &str) {}
    fn send(&mut self, gid: &str, text: &str, _reply: Option<String>) -> Result<String, String> { self.sent.push((gid.into(), text.into())); Ok("m".into()) }
    fn send_file(&mut self, _g: &str, _n: &str, _m: &str, _d: &[u8], _c: &str) -> Result<String, String> { Err("no files".into()) }
    fn oracle_quiet(&self, gid: &str) -> bool { self.quiet.contains(gid) }
    fn set_avatar(&mut self, mime: &str, data: &[u8]) -> Result<(), String> {
        if let Ok(d) = std::env::var("AVATAR_SAMPLE_DIR") { let _ = std::fs::write(std::path::Path::new(&d).join("avatar.png"), data); }
        self.avatars.push((mime.into(), data.len()));
        Ok(())
    }
    fn joined_rooms(&self) -> Vec<String> { self.rooms.clone() }
}

fn cfg() -> Config { Config { avatar_daily: true, avatar_hour: 6, ..Config::default() } }

fn bot_with(dir: &Path, cfg: Config, ollama: &FakeHttp, studio: Arc<FakeStudio>, gpu: Arc<Gpu>) -> Bot {
    let key = dir.join("deepseek-key");
    std::fs::write(&key, "k\n").unwrap();
    Bot::new(
        cfg,
        Box::new(Ollama::new(&ollama.url(), None, Duration::from_secs(5))),
        Box::new(DeepSeek::new("http://127.0.0.1:1", key, "deepseek-chat", Duration::from_secs(5))),
        Arc::new(DeadSearch),
        studio,
        gpu,
        dir.join("state.json"),
    )
}
fn free() -> Arc<Gpu> { Arc::new(Gpu(AtomicBool::new(false))) }
fn asks(o: &FakeHttp) -> usize { o.hits("/api/chat") }

#[test]
fn once_a_day_across_a_restart_and_again_the_next_day() {
    let dir = tempfile::tempdir().unwrap();
    let ollama = fake_ollama(&["qwen3.5:9b"], GOOD);
    let studio = FakeStudio::new();
    let bot = bot_with(dir.path(), cfg(), &ollama, studio.clone(), free());
    let mut chat = FakeChat::new();

    assert_eq!(bot.avatar_try_at(&mut chat, 20_000, 5), AvatarOutcome::NotYet);
    assert_eq!(asks(&ollama), 0);
    assert_eq!(bot.avatar_try_at(&mut chat, 20_000, 7), AvatarOutcome::Set { announced: 2 });
    assert_eq!(chat.avatars.len(), 1);
    let (mime, size) = chat.avatars[0].clone();
    assert_eq!(mime, "image/png");
    assert!(size as u64 <= MAX_AVATAR, "{size} bytes");
    // the prompt that reached Studio: the idea, framed as a headshot
    let sent = studio.submits.lock().unwrap()[0].clone();
    assert_eq!(sent.kind, Kind::Picture);
    assert_eq!(sent.size, "square");
    assert!(sent.prompt.contains("cursed mango prophet") && sent.prompt.contains("headshot"));
    // one line, in the rooms where he may join in, and not in the quiet one
    let rooms: Vec<&str> = chat.sent.iter().map(|(g, _)| g.as_str()).collect();
    assert_eq!(rooms, ["a", "b"]);
    assert!(chat.sent[0].1.starts_with("Wah gwaan"));

    // same day again, and after a restart (a new Bot reading the same state file): nothing happens
    assert_eq!(bot.avatar_try_at(&mut chat, 20_000, 9), AvatarOutcome::Settled);
    drop(bot);
    let bot2 = bot_with(dir.path(), cfg(), &ollama, studio.clone(), free());
    assert_eq!(bot2.avatar_try_at(&mut chat, 20_000, 23), AvatarOutcome::Settled);
    assert_eq!((asks(&ollama), studio.submitted(), chat.avatars.len()), (1, 1, 1));

    // the next calendar day, a new one
    assert_eq!(bot2.avatar_try_at(&mut chat, 20_001, 6), AvatarOutcome::Set { announced: 2 });
    assert_eq!((asks(&ollama), studio.submitted(), chat.avatars.len()), (2, 2, 2));
}

#[test]
fn off_by_default_and_when_switched_off() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!Config::default().avatar_daily);
    let ollama = fake_ollama(&["qwen3.5:9b"], GOOD);
    let bot = bot_with(dir.path(), Config::default(), &ollama, FakeStudio::new(), free());
    let mut chat = FakeChat::new();
    assert_eq!(bot.avatar_try_at(&mut chat, 1, 12), AvatarOutcome::Disabled);
    assert_eq!(bot.avatar_tick(&mut chat), AvatarOutcome::Disabled);
    assert_eq!(asks(&ollama), 0);
}

#[test]
fn a_busy_gpu_defers_without_spending_anything_and_it_goes_through_later_that_day() {
    let dir = tempfile::tempdir().unwrap();
    let ollama = fake_ollama(&["qwen3.5:9b"], GOOD);
    let studio = FakeStudio::new();
    let gpu = Arc::new(Gpu(AtomicBool::new(true)));
    let bot = bot_with(dir.path(), cfg(), &ollama, studio.clone(), gpu.clone());
    let mut chat = FakeChat::new();
    for _ in 0..5 {
        assert!(matches!(bot.avatar_try_at(&mut chat, 5, 8), AvatarOutcome::Deferred(_)));
    }
    studio.game.store(true, Ordering::SeqCst);
    gpu.0.store(false, Ordering::SeqCst);
    assert!(matches!(bot.avatar_try_at(&mut chat, 5, 9), AvatarOutcome::Deferred(r) if r.contains("game")));
    assert_eq!((asks(&ollama), studio.submitted()), (0, 0));
    studio.game.store(false, Ordering::SeqCst);
    assert!(matches!(bot.avatar_try_at(&mut chat, 5, 10), AvatarOutcome::Set { .. }));
    // and a day that never frees up is simply skipped: the next day starts fresh
    let busy = Arc::new(Gpu(AtomicBool::new(true)));
    let dir2 = tempfile::tempdir().unwrap();
    let bot2 = bot_with(dir2.path(), cfg(), &ollama, studio.clone(), busy);
    assert!(matches!(bot2.avatar_try_at(&mut chat, 6, 23), AvatarOutcome::Deferred(_)));
    assert!(matches!(bot2.avatar_try_at(&mut chat, 7, 6), AvatarOutcome::Deferred(_)));
}

#[test]
fn an_idea_that_trips_the_safety_rules_skips_the_day() {
    for bad in [
        "LOOK: a naked kid with a crown\nLINE: wah gwaan",
        "LOOK: a bloody severed head on a plate\nLINE: wah gwaan",
        "LOOK: a sexy nude celebrity\nLINE: wah gwaan",
        "LOOK: a calm moth priest\nLINE: show me the nude kid again",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let ollama = fake_ollama(&["qwen3.5:9b"], bad);
        let studio = FakeStudio::new();
        let bot = bot_with(dir.path(), cfg(), &ollama, studio.clone(), free());
        let mut chat = FakeChat::new();
        assert!(matches!(bot.avatar_try_at(&mut chat, 9, 7), AvatarOutcome::Skipped(_)), "{bad}");
        assert_eq!(bot.avatar_try_at(&mut chat, 9, 8), AvatarOutcome::Settled);
        assert_eq!((studio.submitted(), chat.avatars.len(), chat.sent.len()), (0, 0, 0));
    }
}

#[test]
fn studio_failing_is_retried_a_few_times_and_then_left_alone_until_tomorrow() {
    let dir = tempfile::tempdir().unwrap();
    let ollama = fake_ollama(&["qwen3.5:9b"], GOOD);
    let studio = FakeStudio::new();
    studio.job_fails.store(true, Ordering::SeqCst);
    let bot = bot_with(dir.path(), cfg(), &ollama, studio.clone(), free());
    let mut chat = FakeChat::new();
    for _ in 0..3 { assert!(matches!(bot.avatar_try_at(&mut chat, 3, 7), AvatarOutcome::Failed(_))); }
    assert_eq!(bot.avatar_try_at(&mut chat, 3, 8), AvatarOutcome::GaveUp);
    assert_eq!(studio.submitted(), 3);
    assert!(chat.avatars.is_empty() && chat.sent.is_empty());
    studio.job_fails.store(false, Ordering::SeqCst);
    assert!(matches!(bot.avatar_try_at(&mut chat, 4, 6), AvatarOutcome::Set { .. }));
}

#[test]
fn a_file_that_is_not_a_picture_never_reaches_the_core() {
    let dir = tempfile::tempdir().unwrap();
    let ollama = fake_ollama(&["qwen3.5:9b"], GOOD);
    let studio = FakeStudio::new();
    studio.garbage.store(true, Ordering::SeqCst);
    let bot = bot_with(dir.path(), cfg(), &ollama, studio, free());
    let mut chat = FakeChat::new();
    assert!(matches!(bot.avatar_try_at(&mut chat, 3, 7), AvatarOutcome::Failed(_)));
    assert!(chat.avatars.is_empty());
}

#[test]
fn the_studio_daily_cap_is_respected_and_the_model_failing_to_answer_in_form_is_a_failure() {
    let dir = tempfile::tempdir().unwrap();
    let ollama = fake_ollama(&["qwen3.5:9b"], GOOD);
    let studio = FakeStudio::new();
    let bot = bot_with(dir.path(), Config { studio_daily_cap: 0, ..cfg() }, &ollama, studio.clone(), free());
    let mut chat = FakeChat::new();
    assert!(matches!(bot.avatar_try_at(&mut chat, 3, 7), AvatarOutcome::Skipped(_)));
    assert_eq!(studio.submitted(), 0);

    let dir = tempfile::tempdir().unwrap();
    let rambling = fake_ollama(&["qwen3.5:9b"], "I think a nice picture would be good, mon.");
    let bot = bot_with(dir.path(), cfg(), &rambling, studio.clone(), free());
    assert!(matches!(bot.avatar_try_at(&mut chat, 3, 7), AvatarOutcome::Failed(_)));
    assert_eq!(studio.submitted(), 0);
}

#[test]
fn no_announcement_when_switched_off_and_the_picture_is_still_set() {
    let dir = tempfile::tempdir().unwrap();
    let ollama = fake_ollama(&["qwen3.5:9b"], GOOD);
    let bot = bot_with(dir.path(), Config { avatar_announce: false, ..cfg() }, &ollama, FakeStudio::new(), free());
    let mut chat = FakeChat::new();
    assert_eq!(bot.avatar_try_at(&mut chat, 3, 7), AvatarOutcome::Set { announced: 0 });
    assert_eq!((chat.avatars.len(), chat.sent.len()), (1, 0));
}

/// One real run against the local Ollama and Studio, for a human to look at. Not part of the normal suite:
/// `AVATAR_SAMPLE_DIR=... cargo test -p commons-bot --test avatar real_sample -- --ignored --nocapture`.
/// It never touches an account: the picture and the idea are written into the folder.
#[test]
#[ignore]
fn real_sample() {
    use commons_bot::gpu::GpuGate;
    use commons_bot::studio::HttpStudio;
    let out = std::path::PathBuf::from(std::env::var("AVATAR_SAMPLE_DIR").expect("AVATAR_SAMPLE_DIR"));
    std::fs::create_dir_all(&out).unwrap();
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("k");
    std::fs::write(&key, "k").unwrap();
    let model = std::env::var("MEGATRON_BOT_MODEL").ok();
    let bot = Bot::new(
        cfg(),
        Box::new(Ollama::new("http://127.0.0.1:11434", model.as_deref(), Duration::from_secs(180))),
        Box::new(DeepSeek::new("http://127.0.0.1:1", key, "deepseek-chat", Duration::from_secs(5))),
        Arc::new(DeadSearch),
        Arc::new(HttpStudio::new("http://127.0.0.1:4576", &home.join(".config/homebase/token"))),
        Arc::new(GpuGate::from_env()),
        dir.path().join("state.json"),
    );
    let mut chat = FakeChat::new();
    let outcome = bot.avatar_try_at(&mut chat, 1, 12);
    println!("outcome: {outcome:?}");
    println!("state: {}", std::fs::read_to_string(dir.path().join("state.json")).unwrap_or_default());
    println!("sent: {:?}", chat.sent);
    if let Some((_, n)) = chat.avatars.first() { println!("avatar bytes: {n}"); }
    assert!(matches!(outcome, AvatarOutcome::Set { .. }) || std::env::var("AVATAR_SAMPLE_OK_IF_DEFERRED").is_ok());
}
