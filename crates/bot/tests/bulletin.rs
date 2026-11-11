//! The daily Babylon bulletin, end to end through the local relay and against a chat that only remembers. The real Ollama
//! and DuckDuckGo are never contacted: a scripted model and a fake results page stand in.
mod common;
use common::*;
use commons_bot::bot::{Bot, Chat, Config, Job, JobKind, Line};
use commons_bot::bulletin::{self, Schedule, DEFAULT_MINUTE};
use commons_bot::gpu::{GpuCheck, Pause};
use commons_bot::llm::{DeepSeek, Ollama};
use commons_bot::search::{DuckDuckGo, WebSearch};
use commons_core::state::{MsgBody, Role};
use serde_json::json;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// New York in winter: five hours behind UTC.
const OFFSET: i32 = -5 * 3600;
/// A local day to run the tests on (2026-11-10, a Tuesday).
const DAY: i64 = 20_767;
/// The UTC seconds at `h:m` local on local day `day`.
fn at(day: i64, h: u64, m: u64) -> u64 { (day as u64) * 86_400 + h * 3600 + m * 60 + (-OFFSET) as u64 }

/// A fake Ollama that answers each chat call with the next of `replies` (the last one repeats).
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

/// A DuckDuckGo results page with these `(title, url, snippet)` results.
fn ddg_html(results: &[(&str, &str, &str)]) -> String {
    let mut h = String::from("<html><body><div id=\"links\">");
    for (title, url, snippet) in results {
        h.push_str(&format!(
            "<div class=\"result\"><h2 class=\"result__title\"><a class=\"result__a\" href=\"{url}\">{title}</a></h2>\
             <a class=\"result__snippet\" href=\"{url}\">{snippet}</a></div>"
        ));
    }
    h.push_str("</div></body></html>");
    h
}

const COMET: (&str, &str, &str) = ("Comet with two tails spotted", "https://science.example.com/2026/11/comet-two-tails", "Astronomers spotted a comet with two tails over the weekend.");
const ROBOT: (&str, &str, &str) = ("Robot dog learns to climb stairs", "https://tech.example.org/news/robot-dog-stairs", "A university team taught a four-legged robot to climb a spiral staircase.");
const FINAL: (&str, &str, &str) = ("Local team wins 3-1 in the final", "https://sports.example.net/story/final-3-1", "The home side won the cup final 3-1 on Saturday night, with Sam Okafor scoring twice.");
const GRIM: (&str, &str, &str) = ("Overdose deaths rise again", "https://news.example.com/2026/11/overdose-deaths-rise", "Officials said overdose deaths rose.");
const FRONT: (&str, &str, &str) = ("Science news", "https://frontpage.example.com/", "All the news.");

/// Every query gets the same page: three usable stories on three sites, one too grim to joke about and a front page.
fn good_search() -> FakeHttp { fake_search(&ddg_html(&[GRIM, FRONT, COMET, ROBOT, FINAL])) }

const REPLY: &str = "NEWS 1: Astronomers spot a comet wid two tail over di weekend, Jah know.\n\
    THEORY 1: Two tail mean two Babylon antenna. Dem a beam di weather script straight into yuh fridge.\n\
    NEWS 2: Some university team teach a four-leg robot fi climb a spiral staircase.\n\
    THEORY 2: Robot dog a climb stairs because di pigeons too lazy fi fly up to di Illuminati penthouse now.\n\
    NEWS 3: Di home side win di cup final 3-1 Saturday night.\n\
    THEORY 3: 3-1 add up to 4, an four a di number of fridge magnets Babylon use fi hold up di moon projector.";

fn make_bot(dir: &Path, cfg: Config, ollama_url: &str, search: Arc<dyn WebSearch>, gpu: Arc<dyn GpuCheck>) -> Bot {
    let key = dir.join("deepseek-key");
    std::fs::write(&key, "test-key\n").unwrap();
    Bot::new(
        cfg,
        Box::new(Ollama::new(ollama_url, None, Duration::from_secs(5))),
        Box::new(DeepSeek::new("http://127.0.0.1:1", key, "deepseek-chat", Duration::from_secs(5))),
        search,
        Arc::new(DeadStudio),
        gpu,
        dir.join("state.json"),
    )
}

/// No post limit to speak of, and no typing pauses.
fn quick() -> Config { Config { max_per_minute: 1000, chime_delay_secs: (0, 0), ..Config::default() } }

fn ddg(url: &str) -> Arc<dyn WebSearch> { Arc::new(DuckDuckGo::with_limits(url, Duration::from_secs(5), 1000, 5, 200)) }

/// A GPU that is busy when the flag says so.
struct ToggleGpu(AtomicBool);
impl GpuCheck for ToggleGpu {
    fn blocked(&self) -> Option<Pause> { if self.0.load(Ordering::SeqCst) { Some(Pause::Game("dwu".into())) } else { None } }
}

fn from_oracle(p: &Peer, gid: &str, oracle: &str) -> Vec<(String, String)> {
    p.c.messages(gid).into_iter().filter(|m| m.sender == oracle && !m.deleted)
        .filter_map(|m| match m.body { MsgBody::Text(t) => Some((m.id, t)), _ => None }).collect()
}

fn bulletin_jobs(bot: &Bot, c: &mut commons_core::client::Client) -> Vec<Job> {
    bot.poll(c).into_iter().filter(|j| j.kind == JobKind::Bulletin).collect()
}

// ---------------------------------------------------------------- the phrases

#[test]
fn only_the_owner_or_an_admin_can_set_the_bulletin_and_the_time_is_remembered() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut bob = Peer::new(&net, "bob");
    let mut carol = Peer::new(&net, "carol");
    let mut oracle = Peer::new(&net, "Oracle");
    let (cid, gen) = community_with(&net, &mut alice, &mut [&mut bob, &mut carol, &mut oracle]);
    alice.c.set_role(&cid, &carol.acct(), Role::Admin).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut carol, &mut oracle]);

    let ollama = scripted_ollama(&["unused"]);
    let dir = tempfile::tempdir().unwrap();
    let bot = make_bot(dir.path(), quick(), &ollama.url(), Arc::new(DeadSearch), Arc::new(NoGpu));
    // Early in the morning, so no chosen time has passed yet today.
    bot.set_clock(at(DAY, 7, 0), OFFSET);
    bot.poll(&mut oracle.c);
    macro_rules! all {
        ($text:expr, $who:ident) => {{
            $who.c.send_text(&gen, $text, None).unwrap();
            settle(&mut [&mut alice, &mut bob, &mut carol, &mut oracle]);
            bot.poll(&mut oracle.c)
        }};
    }

    // A member: refused in character, nothing stored.
    let jobs = all!("@Oracle bulletin here", bob);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].kind, JobKind::BulletinNotAllowed);
    let sent = bot.answer(&mut oracle.c, &jobs[0]).unwrap();
    assert_eq!(sent.text, bulletin::NOT_ALLOWED_REPLY);
    assert!(bot.bulletin_schedule(&gen).is_none());
    let jobs = all!("@Oracle stop bulletin", bob);
    assert_eq!(jobs[0].kind, JobKind::BulletinNotAllowed);

    // A sentence that merely contains the words is a question, not the phrase.
    let jobs = all!("@Oracle can you do a bulletin here please", alice);
    assert_eq!(jobs[0].kind, JobKind::Answer { deepseek: false });
    assert!(bot.bulletin_schedule(&gen).is_none());

    // The owner sets a time, in the evening form.
    let jobs = all!("@Oracle bulletin at 8pm", alice);
    assert_eq!(jobs[0].kind, JobKind::SetBulletin { on: true, minute: 20 * 60 });
    let sent = bot.answer(&mut oracle.c, &jobs[0]).unwrap();
    assert!(sent.text.contains("8:00 PM") && !sent.text.contains("tomorrow"), "{}", sent.text);
    assert_eq!(bot.bulletin_schedule(&gen).map(|s| s.minute), Some(20 * 60));

    // A time that has already passed today starts tomorrow, and does not fire the moment it is switched on.
    bot.set_clock(at(DAY, 21, 0), OFFSET);
    let jobs = all!("@Oracle bulletin at 8pm", alice);
    assert_eq!(jobs.len(), 1, "no bulletin job alongside the switch: {jobs:?}");
    assert!(bot.answer(&mut oracle.c, &jobs[0]).unwrap().text.contains("8:00 PM, starting tomorrow"));
    assert_eq!(bot.bulletin_schedule(&gen).unwrap().last_day, DAY);
    assert!(bulletin_jobs(&bot, &mut oracle.c).is_empty());
    bot.set_clock(at(DAY, 7, 0), OFFSET);

    // A time it cannot read changes nothing and says so.
    let jobs = all!("@Oracle bulletin at teatime", alice);
    assert_eq!(jobs[0].kind, JobKind::BulletinBadTime);
    assert_eq!(bot.answer(&mut oracle.c, &jobs[0]).unwrap().text, bulletin::BAD_TIME_REPLY);
    assert_eq!(bot.bulletin_schedule(&gen).map(|s| s.minute), Some(20 * 60));

    // It survives a restart of the bot, and "here" keeps the time already chosen.
    let again = make_bot(dir.path(), quick(), &ollama.url(), Arc::new(DeadSearch), Arc::new(NoGpu));
    assert_eq!(again.bulletin_schedule(&gen).map(|s| s.minute), Some(20 * 60));
    drop(again);
    let jobs = all!("@Oracle bulletin here", alice);
    assert_eq!(jobs[0].kind, JobKind::SetBulletin { on: true, minute: 20 * 60 });

    // An admin may change it too, in the 24-hour form.
    let jobs = all!("@Oracle bulletin at 07:30", carol);
    assert_eq!(jobs[0].kind, JobKind::SetBulletin { on: true, minute: 7 * 60 + 30 });
    assert!(bot.answer(&mut oracle.c, &jobs[0]).unwrap().text.contains("7:30 AM"));

    // Off, by an admin; and "here" afterwards starts from the default time.
    let jobs = all!("@Oracle stop bulletin", carol);
    assert_eq!(jobs[0].kind, JobKind::SetBulletin { on: false, minute: 0 });
    assert_eq!(bot.answer(&mut oracle.c, &jobs[0]).unwrap().text, bulletin::OFF_REPLY);
    assert!(bot.bulletin_schedule(&gen).is_none());
    let jobs = all!("@Oracle bulletin here", alice);
    assert_eq!(jobs[0].kind, JobKind::SetBulletin { on: true, minute: DEFAULT_MINUTE });
    assert_eq!(bot.bulletin_schedule(&gen), Some(Schedule { minute: DEFAULT_MINUTE, last_day: 0, theories: vec![] }), "off and on again forgets the old day");
}

// ---------------------------------------------------------------- the daily post and the vote

#[test]
fn the_bulletin_goes_out_once_a_day_with_real_links_numbered_theories_and_yesterdays_winner() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut bob = Peer::new(&net, "bob");
    let mut oracle = Peer::new(&net, "Oracle");
    let (_cid, gen) = community_with(&net, &mut alice, &mut [&mut bob, &mut oracle]);
    let me = oracle.acct();

    let ollama = scripted_ollama(&[REPLY]);
    let search = good_search();
    let dir = tempfile::tempdir().unwrap();
    let bot = make_bot(dir.path(), quick(), &ollama.url(), ddg(&search.url()), Arc::new(NoGpu));
    bot.set_clock(at(DAY, 7, 0), OFFSET);
    bot.poll(&mut oracle.c);
    alice.c.send_text(&gen, "@Oracle bulletin here", None).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut oracle]);
    let jobs = bot.poll(&mut oracle.c);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].kind, JobKind::SetBulletin { on: true, minute: DEFAULT_MINUTE });
    bot.answer(&mut oracle.c, &jobs[0]).unwrap();

    // Not before its time; at its time exactly one job, and not a second while that one is being made.
    bot.set_clock(at(DAY, 8, 59), OFFSET);
    assert!(bulletin_jobs(&bot, &mut oracle.c).is_empty());
    bot.set_clock(at(DAY, 9, 0), OFFSET);
    let jobs = bulletin_jobs(&bot, &mut oracle.c);
    assert_eq!(jobs.len(), 1);
    assert!(bulletin_jobs(&bot, &mut oracle.c).is_empty(), "one bulletin at a time per room");
    let sent = bot.answer(&mut oracle.c, &jobs[0]).expect("the bulletin was posted");
    assert!(sent.text.contains("First Babylon Bulletin"), "{}", sent.text);
    settle(&mut [&mut alice, &mut bob, &mut oracle]);

    // What the room sees: the opener, then for each story its straight line with the real link and its numbered theory.
    let posts = from_oracle(&alice, &gen, &me);
    let texts: Vec<&str> = posts.iter().skip(1).map(|(_, t)| t.as_str()).collect();
    assert_eq!(posts.len(), 1 + 1 + 3 * 2, "{texts:#?}");
    assert_eq!(posts[1].1, sent.text);
    assert!(texts[1].starts_with("1. Astronomers spot a comet") && texts[1].ends_with(COMET.1), "{}", texts[1]);
    assert!(texts[2].starts_with("Theory #1: Two tail mean two Babylon antenna"), "{}", texts[2]);
    assert!(texts[3].starts_with("2. ") && texts[3].ends_with(ROBOT.1), "{}", texts[3]);
    assert!(texts[4].starts_with("Theory #2: Robot dog"), "{}", texts[4]);
    assert!(texts[5].starts_with("3. Di home side win di cup final 3-1") && texts[5].ends_with(FINAL.1), "{}", texts[5]);
    assert!(texts[6].starts_with("Theory #3: 3-1 add up to 4"), "{}", texts[6]);
    for t in &texts { assert!(!t.contains("highway") && !t.contains("frontpage"), "heavy news and front pages stay out: {t}"); }
    let disk = relay_bytes(&net);
    assert!(!contains(&disk, "Babylon antenna") && !contains(&disk, "comet-two-tails"), "only ciphertext reaches the relay");
    // The model saw the real items, quoted, and nothing it wrote was asked twice.
    let chats: Vec<_> = ollama.requests().into_iter().filter(|r| r.path == "/api/chat").collect();
    assert_eq!(chats.len(), 1);
    let body = chats[0].json();
    let user = body["messages"].as_array().unwrap().iter().find(|m| m["role"] == "user").unwrap()["content"].as_str().unwrap();
    assert!(user.contains("Comet with two tails spotted") && user.contains("quoted data from the web"), "{user}");
    assert!(body["messages"][0]["content"].as_str().unwrap().contains("NEWS <number>"));
    let theories: Vec<&(String, String)> = posts.iter().filter(|(_, t)| t.starts_with("Theory #")).collect();
    assert_eq!(bot.bulletin_schedule(&gen).unwrap().theories.iter().map(|t| t.id.clone()).collect::<Vec<_>>(), theories.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>());

    // Never twice in a day: not later the same day, and not from a fresh process with the same state.
    bot.set_clock(at(DAY, 9, 30), OFFSET);
    assert!(bulletin_jobs(&bot, &mut oracle.c).is_empty());
    let restarted = make_bot(dir.path(), quick(), &ollama.url(), ddg(&search.url()), Arc::new(NoGpu));
    restarted.set_clock(at(DAY, 12, 0), OFFSET);
    assert!(bulletin_jobs(&restarted, &mut oracle.c).is_empty());
    assert_eq!(restarted.bulletin_schedule(&gen).unwrap().last_day, DAY);

    // The vote: two people react to Theory #2 (three reactions in all), one to Theory #1; the bot's own reaction on #3 never counts.
    let (t1, t2, t3) = (&theories[0].0, &theories[1].0, &theories[2].0);
    alice.c.react(&gen, t2, "😂", true).unwrap();
    alice.c.react(&gen, t2, "🔥", true).unwrap();
    bob.c.react(&gen, t2, "😂", true).unwrap();
    alice.c.react(&gen, t1, "😂", true).unwrap();
    oracle.c.react(&gen, t3, "😂", true).unwrap();
    oracle.c.react(&gen, t3, "🔥", true).unwrap();
    oracle.c.react(&gen, t3, "👀", true).unwrap();
    oracle.c.react(&gen, t3, "💀", true).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut oracle]);

    // The next day's bulletin names yesterday's dumbest theory.
    restarted.set_clock(at(DAY + 1, 9, 0), OFFSET);
    let jobs = bulletin_jobs(&restarted, &mut oracle.c);
    assert_eq!(jobs.len(), 1);
    let sent = restarted.answer(&mut oracle.c, &jobs[0]).unwrap();
    assert!(sent.text.contains("Theory #2 wid 3 reactions") && sent.text.contains("Robot dog a climb stairs"), "{}", sent.text);
    settle(&mut [&mut alice, &mut bob, &mut oracle]);
    let posts = from_oracle(&alice, &gen, &me);
    assert_eq!(posts.len(), 1 + 7 * 2);

    // A tie keeps both; no votes at all makes him sulk.
    let theories: Vec<String> = posts.iter().rev().filter(|(_, t)| t.starts_with("Theory #")).take(3).map(|(id, _)| id.clone()).collect::<Vec<_>>().into_iter().rev().collect();
    alice.c.react(&gen, &theories[0], "😂", true).unwrap();
    bob.c.react(&gen, &theories[2], "😂", true).unwrap();
    settle(&mut [&mut alice, &mut bob, &mut oracle]);
    restarted.set_clock(at(DAY + 2, 9, 0), OFFSET);
    let jobs = bulletin_jobs(&restarted, &mut oracle.c);
    let sent = restarted.answer(&mut oracle.c, &jobs[0]).unwrap();
    assert!(sent.text.contains("Theory #1 an Theory #3, 1 reaction each"), "{}", sent.text);
    settle(&mut [&mut alice, &mut bob, &mut oracle]);
    restarted.set_clock(at(DAY + 3, 9, 0), OFFSET);
    let jobs = bulletin_jobs(&restarted, &mut oracle.c);
    let sent = restarted.answer(&mut oracle.c, &jobs[0]).unwrap();
    assert!(sent.text.contains("Nobody vote") && sent.text.contains("Mi vex"), "{}", sent.text);
    assert_eq!(commons_bot::bot::no_emoji(&sent.text), sent.text);
}

// ---------------------------------------------------------------- the GPU and the quiet room

#[test]
fn a_busy_gpu_makes_the_bulletin_wait_and_a_block_that_outlasts_the_window_skips_the_day() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut oracle = Peer::new(&net, "Oracle");
    let (_cid, gen) = community_with(&net, &mut alice, &mut [&mut oracle]);
    let me = oracle.acct();

    let ollama = scripted_ollama(&[REPLY]);
    let search = good_search();
    let gpu = Arc::new(ToggleGpu(AtomicBool::new(true)));
    let dir = tempfile::tempdir().unwrap();
    let bot = make_bot(dir.path(), quick(), &ollama.url(), ddg(&search.url()), gpu.clone());
    bot.set_clock(at(DAY, 7, 0), OFFSET);
    bot.poll(&mut oracle.c);
    alice.c.send_text(&gen, "@Oracle bulletin here", None).unwrap();
    settle(&mut [&mut alice, &mut oracle]);
    assert_eq!(bot.poll(&mut oracle.c).len(), 1);

    // A game has the card at nine: nothing, and nothing a minute later either.
    bot.set_clock(at(DAY, 9, 0), OFFSET);
    assert!(bulletin_jobs(&bot, &mut oracle.c).is_empty());
    bot.set_clock(at(DAY, 9, 1), OFFSET);
    assert!(bulletin_jobs(&bot, &mut oracle.c).is_empty());
    assert_eq!(ollama.hits("/api/chat"), 0);
    // The game ends at ten: the bulletin goes out late rather than not at all.
    gpu.0.store(false, Ordering::SeqCst);
    bot.set_clock(at(DAY, 10, 0), OFFSET);
    let jobs = bulletin_jobs(&bot, &mut oracle.c);
    assert_eq!(jobs.len(), 1);
    // ... unless the card is taken again in between: then the job gives up silently and a later poll makes a new one.
    gpu.0.store(true, Ordering::SeqCst);
    assert!(bot.answer(&mut oracle.c, &jobs[0]).is_none());
    gpu.0.store(false, Ordering::SeqCst);
    bot.set_clock(at(DAY, 10, 1), OFFSET);
    let jobs = bulletin_jobs(&bot, &mut oracle.c);
    assert_eq!(jobs.len(), 1, "the room is free again for a new attempt");
    assert!(bot.answer(&mut oracle.c, &jobs[0]).is_some());
    settle(&mut [&mut alice, &mut oracle]);
    assert_eq!(from_oracle(&alice, &gen, &me).len(), 7, "the switch was never answered in this test; the bulletin alone");

    // The next day the game runs past the whole window: that day is skipped, quietly, and the day after is normal.
    gpu.0.store(true, Ordering::SeqCst);
    bot.set_clock(at(DAY + 1, 9, 0), OFFSET);
    assert!(bulletin_jobs(&bot, &mut oracle.c).is_empty());
    bot.set_clock(at(DAY + 1, 13, 1), OFFSET);
    assert!(bulletin_jobs(&bot, &mut oracle.c).is_empty());
    assert_eq!(bot.bulletin_schedule(&gen).unwrap().last_day, DAY + 1, "the day is spent");
    gpu.0.store(false, Ordering::SeqCst);
    bot.set_clock(at(DAY + 1, 13, 5), OFFSET);
    assert!(bulletin_jobs(&bot, &mut oracle.c).is_empty(), "too late today");
    bot.set_clock(at(DAY + 2, 9, 0), OFFSET);
    assert_eq!(bulletin_jobs(&bot, &mut oracle.c).len(), 1);
    settle(&mut [&mut alice, &mut oracle]);
    assert_eq!(from_oracle(&alice, &gen, &me).len(), 7, "nothing was posted on the skipped day");
}

#[test]
fn a_room_the_owner_set_quiet_gets_no_bulletin() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "alice");
    let mut oracle = Peer::new(&net, "Oracle");
    let (cid, gen) = community_with(&net, &mut alice, &mut [&mut oracle]);
    let me = oracle.acct();

    let ollama = scripted_ollama(&[REPLY]);
    let search = good_search();
    let dir = tempfile::tempdir().unwrap();
    let bot = make_bot(dir.path(), quick(), &ollama.url(), ddg(&search.url()), Arc::new(NoGpu));
    bot.set_clock(at(DAY, 7, 0), OFFSET);
    bot.poll(&mut oracle.c);
    alice.c.set_oracle_quiet(&cid, &gen, true).unwrap();
    alice.c.send_text(&gen, "@Oracle bulletin here", None).unwrap();
    settle(&mut [&mut alice, &mut oracle]);
    let jobs = bot.poll(&mut oracle.c);
    assert_eq!(jobs[0].kind, JobKind::SetBulletin { on: true, minute: DEFAULT_MINUTE });
    assert_eq!(bot.answer(&mut oracle.c, &jobs[0]).unwrap().text, bulletin::ON_REPLY_QUIET, "the switch is remembered, and he says why nothing will come");

    bot.set_clock(at(DAY, 9, 0), OFFSET);
    assert!(bulletin_jobs(&bot, &mut oracle.c).is_empty());
    bot.set_clock(at(DAY, 13, 1), OFFSET);
    assert!(bulletin_jobs(&bot, &mut oracle.c).is_empty());
    assert_eq!(bot.bulletin_schedule(&gen).unwrap().last_day, DAY);
    assert_eq!(ollama.hits("/api/chat"), 0, "the model was never asked");
    assert_eq!(search.requests().len(), 0, "nothing was searched");

    // The owner lifts quiet: the next day's bulletin comes.
    alice.c.set_oracle_quiet(&cid, &gen, false).unwrap();
    settle(&mut [&mut alice, &mut oracle]);
    bot.set_clock(at(DAY + 1, 9, 0), OFFSET);
    let jobs = bulletin_jobs(&bot, &mut oracle.c);
    assert_eq!(jobs.len(), 1);
    assert!(bot.answer(&mut oracle.c, &jobs[0]).is_some());
    settle(&mut [&mut alice, &mut oracle]);
    assert_eq!(from_oracle(&alice, &gen, &me).len(), 1 + 7);
}

// ---------------------------------------------------------------- the filters, against a chat that only remembers

struct MemChat { sent: Vec<String>, quiet: bool, reactions: Vec<(String, usize)> }
impl MemChat { fn new() -> MemChat { MemChat { sent: vec![], quiet: false, reactions: vec![] } } }
impl Chat for MemChat {
    fn my_account(&self) -> String { "bot".into() }
    fn history(&self, _: &str, _: usize) -> Vec<Line> { vec![] }
    fn typing(&mut self, _: &str) {}
    fn send(&mut self, _: &str, text: &str, reply: Option<String>) -> Result<String, String> {
        assert!(reply.is_none(), "a bulletin answers nobody");
        self.sent.push(text.to_string());
        Ok(format!("m{}", self.sent.len()))
    }
    fn send_file(&mut self, _: &str, _: &str, _: &str, _: &[u8], _: &str) -> Result<String, String> { Err("no files in a bulletin".into()) }
    fn oracle_quiet(&self, _: &str) -> bool { self.quiet }
    fn reaction_count(&self, _: &str, id: &str) -> usize { self.reactions.iter().find(|(i, _)| i == id).map_or(0, |(_, n)| *n) }
}
fn bulletin_job() -> Job { Job { gid: "room".into(), trigger: String::new(), from: String::new(), text: String::new(), kind: JobKind::Bulletin } }

fn filter_bot(dir: &Path, ollama: &FakeHttp, search: Arc<dyn WebSearch>) -> Bot {
    let bot = make_bot(dir, quick(), &ollama.url(), search, Arc::new(NoGpu));
    bot.set_bulletin("room", Some(Some(DEFAULT_MINUTE)));
    bot.set_clock(at(DAY, 9, 0), OFFSET);
    bot
}

#[test]
fn grim_news_alone_means_no_bulletin_and_no_model_call() {
    let ollama = scripted_ollama(&[REPLY]);
    let search = fake_search(&ddg_html(&[GRIM, FRONT, ("Two children killed in fire", "https://local.example.com/2026/11/children-killed-fire", "Firefighters said two children died.")]));
    let dir = tempfile::tempdir().unwrap();
    let bot = filter_bot(dir.path(), &ollama, ddg(&search.url()));
    let mut chat = MemChat::new();
    assert!(bot.answer(&mut chat, &bulletin_job()).is_none());
    assert!(chat.sent.is_empty());
    assert_eq!(ollama.hits("/api/chat"), 0);
    assert!(search.requests().len() <= 2 * bulletin::MAX_SEARCHES, "a handful of searches, then it gives up: {}", search.requests().len());
    assert_ne!(bot.bulletin_schedule("room").unwrap().last_day, DAY, "the day is not spent; it tries again later");
    // A search that fails outright is the same: silence, no model.
    let dead = make_bot(dir.path(), quick(), &ollama.url(), Arc::new(DeadSearch), Arc::new(NoGpu));
    dead.set_clock(at(DAY, 9, 0), OFFSET);
    assert!(dead.answer(&mut chat, &bulletin_job()).is_none());
    assert!(chat.sent.is_empty());
}

/// Feeds that list the three usable stories (and a grim one and a front page), and a search that must not be needed.
struct FeedsOnly { feeds: AtomicUsize, searches: AtomicUsize }
impl WebSearch for FeedsOnly {
    fn search(&self, _: &str) -> Result<Vec<commons_bot::search::SearchResult>, String> {
        self.searches.fetch_add(1, Ordering::SeqCst);
        Err("no search when the feeds deliver".into())
    }
    fn feed(&self, url: &str) -> Result<Vec<commons_bot::search::SearchResult>, String> {
        assert!(bulletin::FEEDS.iter().any(|(_, u)| u.contains(&url)), "only listed feeds: {url}");
        self.feeds.fetch_add(1, Ordering::SeqCst);
        Ok([GRIM, FRONT, COMET, ROBOT, FINAL].iter()
            .map(|(t, u, s)| commons_bot::search::SearchResult { title: t.to_string(), url: u.to_string(), snippet: s.to_string() })
            .collect())
    }
}

#[test]
fn the_stories_come_from_the_news_feeds_before_any_search() {
    let ollama = scripted_ollama(&[REPLY]);
    let dir = tempfile::tempdir().unwrap();
    let source = Arc::new(FeedsOnly { feeds: AtomicUsize::new(0), searches: AtomicUsize::new(0) });
    let bot = filter_bot(dir.path(), &ollama, source.clone());
    let mut chat = MemChat::new();
    assert!(bot.answer(&mut chat, &bulletin_job()).is_some());
    assert_eq!(source.searches.load(Ordering::SeqCst), 0, "the feeds delivered, so no search");
    assert!(source.feeds.load(Ordering::SeqCst) >= 3);
    let links: Vec<&str> = [COMET.1, ROBOT.1, FINAL.1].to_vec();
    assert!(links.iter().all(|l| chat.sent.iter().any(|t| t.contains(l))), "{:#?}", chat.sent);
    assert!(chat.sent.iter().all(|t| !t.contains(GRIM.1) && !t.contains(FRONT.1)), "the grim story and the front page stay out");
}

#[test]
fn the_hard_block_the_subject_rule_and_the_numbers_rule_read_every_line_the_model_wrote() {
    // Theory 2 is sexual about minors (the hard block), the news line of item 1 invents a number, and theory 3 is fine.
    let reply = "NEWS 1: Dem find 400 comet wid two tail dis weekend. 🦎\n\
        THEORY 1: Two tail a two Babylon antenna.\n\
        NEWS 2: A robot dog learn fi climb stairs.\n\
        THEORY 2: Di robot dog a film naked teenagers fi Babylon.\n\
        NEWS 3: Di home side win di final 3-1.\n\
        THEORY 3: Di scoreboard a run pon fridge magnets.";
    let ollama = scripted_ollama(&[reply]);
    let search = good_search();
    let dir = tempfile::tempdir().unwrap();
    let bot = filter_bot(dir.path(), &ollama, ddg(&search.url()));
    let mut chat = MemChat::new();
    assert!(bot.answer(&mut chat, &bulletin_job()).is_some());
    assert_eq!(chat.sent.len(), 1 + 2 * 2, "{:#?}", chat.sent);
    assert_eq!(chat.sent[1], format!("1. {}\n{}", COMET.0, COMET.1), "a line with an invented number gives way to the real headline");
    assert_eq!(chat.sent[2], "Theory #1: Two tail a two Babylon antenna.");
    assert_eq!(chat.sent[3], format!("2. Di home side win di final 3-1.\n{}", FINAL.1), "the numbers it kept were in the snippet");
    assert_eq!(chat.sent[4], "Theory #2: Di scoreboard a run pon fridge magnets.");
    assert!(chat.sent.iter().all(|t| !t.contains("naked") && !t.contains("robot")), "the blocked item is gone whole: {:#?}", chat.sent);
    assert!(chat.sent.iter().all(|t| commons_bot::bot::no_emoji(t) == *t));
    assert_eq!(ollama.hits("/api/chat"), 1, "two usable items were enough; no second try");
    assert_eq!(bot.bulletin_schedule("room").unwrap().theories.len(), 2);

    // Theories that deny the event, drift onto medicine or name a real person from the story are dropped too; with fewer than
    // two items left, the model is asked once more and then the bulletin is not posted at all today.
    let reply = "NEWS 1: A comet wid two tail.\nTHEORY 1: Dat comet a hoax, it neva happen.\n\
        NEWS 2: A robot dog a climb stairs.\nTHEORY 2: Dem hide di vaccine inna di robot.\n\
        NEWS 3: Di home side win 3-1.\nTHEORY 3: Okafor a Babylon agent wid fridge magnets.";
    let ollama = scripted_ollama(&[reply]);
    let dir = tempfile::tempdir().unwrap();
    let bot = filter_bot(dir.path(), &ollama, ddg(&search.url()));
    let mut chat = MemChat::new();
    assert!(bot.answer(&mut chat, &bulletin_job()).is_none());
    assert!(chat.sent.is_empty());
    assert_eq!(ollama.hits("/api/chat"), 2);
    assert_ne!(bot.bulletin_schedule("room").unwrap().last_day, DAY);

    // A model that answers with prose, or nothing usable: the same silence.
    let ollama = scripted_ollama(&["Wah gwaan, mi cyaan do dat right now."]);
    let dir = tempfile::tempdir().unwrap();
    let bot = filter_bot(dir.path(), &ollama, ddg(&search.url()));
    let mut chat = MemChat::new();
    assert!(bot.answer(&mut chat, &bulletin_job()).is_none());
    assert!(chat.sent.is_empty());

    // A quiet room, seen at the last moment: nothing goes out and the day is not spent.
    let ollama = scripted_ollama(&[REPLY]);
    let dir = tempfile::tempdir().unwrap();
    let bot = filter_bot(dir.path(), &ollama, ddg(&search.url()));
    let mut chat = MemChat::new();
    chat.quiet = true;
    assert!(bot.answer(&mut chat, &bulletin_job()).is_none());
    assert!(chat.sent.is_empty());
    assert_ne!(bot.bulletin_schedule("room").unwrap().last_day, DAY);
}

#[test]
fn votes_are_counted_from_the_stored_theories() {
    let ollama = scripted_ollama(&[REPLY]);
    let search = good_search();
    let dir = tempfile::tempdir().unwrap();
    let bot = filter_bot(dir.path(), &ollama, ddg(&search.url()));
    let mut chat = MemChat::new();
    assert!(bot.answer(&mut chat, &bulletin_job()).is_some());
    assert_eq!(chat.sent.len(), 7, "{:#?}", chat.sent);
    let stored = bot.bulletin_schedule("room").unwrap().theories;
    assert_eq!(stored.iter().map(|t| (t.id.as_str(), t.n)).collect::<Vec<_>>(), vec![("m3", 1), ("m5", 2), ("m7", 3)]);

    // Next day: reactions on the stored ids decide the opener; the new theories replace the old.
    chat.reactions = vec![("m5".into(), 2), ("m7".into(), 5)];
    bot.set_clock(at(DAY + 1, 9, 0), OFFSET);
    let sent = bot.answer(&mut chat, &bulletin_job()).unwrap();
    assert!(sent.text.contains("Theory #3 wid 5 reactions") && sent.text.contains("fridge magnets"), "{}", sent.text);
    assert_eq!(bot.bulletin_schedule("room").unwrap().theories.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), vec!["m10", "m12", "m14"]);
}

/// A sample against the real local model, by hand only: `ORACLE_BULLETIN_ITEMS=<json file> cargo test ... -- --ignored --nocapture`.
/// The file holds `[{"title","url","snippet"}, ...]` real items (the search engine stands in for nothing here: the items are
/// served from the file as a results page, so the model, the filters and the posting are the production path).
#[test]
#[ignore]
fn live_sample_against_the_local_model() {
    let Ok(path) = std::env::var("ORACLE_BULLETIN_ITEMS") else { eprintln!("ORACLE_BULLETIN_ITEMS not set; nothing to do"); return };
    let items: Vec<serde_json::Value> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let owned: Vec<(String, String, String)> = items.iter().map(|i| (i["title"].as_str().unwrap_or("").to_string(), i["url"].as_str().unwrap_or("").to_string(), i["snippet"].as_str().unwrap_or("").to_string())).collect();
    let rows: Vec<(&str, &str, &str)> = owned.iter().map(|(t, u, s)| (t.as_str(), u.as_str(), s.as_str())).collect();
    let search = fake_search(&ddg_html(&rows));
    let model = std::env::var("MEGATRON_BOT_MODEL").ok();
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("deepseek-key");
    std::fs::write(&key, "unused\n").unwrap();
    let bot = Bot::new(
        quick(),
        Box::new(Ollama::new("http://127.0.0.1:11434", model.as_deref(), Duration::from_secs(300))),
        Box::new(DeepSeek::new("http://127.0.0.1:1", key, "deepseek-chat", Duration::from_secs(5))),
        ddg(&search.url()),
        Arc::new(DeadStudio),
        Arc::new(NoGpu),
        dir.path().join("state.json"),
    );
    bot.set_bulletin("room", Some(Some(DEFAULT_MINUTE)));
    bot.set_clock(at(DAY, 9, 0), OFFSET);
    let mut chat = MemChat::new();
    let posted = bot.answer(&mut chat, &bulletin_job());
    println!("\n===== SAMPLE BULLETIN (posted: {}) =====", posted.is_some());
    for (i, t) in chat.sent.iter().enumerate() { println!("--- message {} ---\n{t}", i + 1); }
    println!("===== END =====");
}
