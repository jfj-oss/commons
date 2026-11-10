//! megatron-bot: a Megatron member whose answers come from a local model.
//!
//!   megatron-bot join <invite-link>   create the bot's account (name Oracle) and ask to join a community
//!   megatron-bot run                  serve the rooms it is in (what the systemd user service runs)
//!
//! Data lives in ~/.local/share/megatron-bot (override: MEGATRON_BOT_DATA). The account's passphrase is generated on
//! first use and kept in that folder, readable by its owner only, so the unattended service can unlock it.
use commons_bot::bot::{Bot, Config, Job, JobKind, SharedClient, DEFAULT_NAME};
use commons_bot::gpu::{GpuGate, Pause};
use commons_bot::llm::{DeepSeek, Ollama, DEFAULT_DEEPSEEK_MODEL, DEFAULT_DEEPSEEK_URL, DEFAULT_OLLAMA};
use commons_bot::search::{DuckDuckGo, SearXng, WebSearch, DEFAULT_SEARCH_URL, SEARCH_TIMEOUT};
use commons_bot::studio::{HttpStudio, StudioApi, DEFAULT_STUDIO_URL};
use commons_core::client::Client;
use commons_core::envelope::InviteLink;
use commons_core::transport::{Api, HttpTransport};
use commons_core::vault::KdfParams;
use rand::RngCore;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

fn home() -> PathBuf { std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")) }

fn data_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("MEGATRON_BOT_DATA") { return PathBuf::from(p); }
    let base = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/share"));
    base.join("megatron-bot")
}

fn deepseek_key_file() -> PathBuf {
    std::env::var_os("MEGATRON_BOT_DEEPSEEK_KEY_FILE").map(PathBuf::from).unwrap_or_else(|| home().join(".config/deepseek-key"))
}

/// Studio's shared token. Read once by `HttpStudio`; never logged.
fn studio_token_file() -> PathBuf {
    std::env::var_os("MEGATRON_BOT_STUDIO_TOKEN_FILE").map(PathBuf::from).unwrap_or_else(|| home().join(".config/homebase/token"))
}

fn env_string(name: &str) -> Option<String> { std::env::var(name).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) }
fn env_usize(name: &str, default: usize) -> usize { env_string(name).and_then(|s| s.parse().ok()).unwrap_or(default) }

fn config() -> Config {
    let mut cfg = Config::default();
    if let Some(n) = env_string("MEGATRON_BOT_NAME") { cfg.name = n; }
    cfg.max_reply_chars = env_usize("MEGATRON_BOT_MAX_REPLY_CHARS", cfg.max_reply_chars).clamp(80, 8000);
    cfg.context_messages = env_usize("MEGATRON_BOT_CONTEXT_MESSAGES", cfg.context_messages).clamp(2, 100);
    cfg.max_per_minute = env_usize("MEGATRON_BOT_MAX_PER_MINUTE", cfg.max_per_minute).max(1);
    cfg.max_queue = env_usize("MEGATRON_BOT_MAX_QUEUE", cfg.max_queue).max(1);
    cfg.studio_daily_cap = env_usize("MEGATRON_BOT_STUDIO_DAILY_CAP", cfg.studio_daily_cap);
    cfg.studio_daily_video_cap = env_usize("MEGATRON_BOT_STUDIO_DAILY_VIDEO_CAP", cfg.studio_daily_video_cap)
        .min(cfg.studio_daily_cap);
    cfg.studio_queue_max = env_usize("MEGATRON_BOT_STUDIO_QUEUE_MAX", cfg.studio_queue_max);
    cfg.chime_chance_pct = env_usize("MEGATRON_BOT_CHIME_PERCENT", 8).min(100);
    cfg.chime_min_gap_secs = env_usize("MEGATRON_BOT_CHIME_GAP_SECS", cfg.chime_min_gap_secs as usize) as u64;
    cfg.art_chance_pct = env_usize("MEGATRON_BOT_ART_PERCENT", 20).min(100);
    cfg.art_daily_max = env_usize("MEGATRON_BOT_ART_DAILY", cfg.art_daily_max as usize) as u32;
    cfg.wall_chance_pct = env_usize("MEGATRON_BOT_WALL_PERCENT", 3).min(100);
    cfg.wall_daily_max = env_usize("MEGATRON_BOT_WALL_DAILY", cfg.wall_daily_max as usize) as u32;
    cfg
}

/// The passphrase that unlocks the bot's own account. There is no one to type it, so it is generated once and kept
/// beside the account, mode 0600. It protects the folder at rest; the account itself is the bot's, not the owner's.
fn passphrase(dir: &Path, create: bool) -> Result<Option<String>, String> {
    let path = dir.join("passphrase");
    if let Ok(s) = std::fs::read_to_string(&path) {
        let s = s.trim().to_string();
        if s.len() >= 8 { return Ok(Some(s)); }
    }
    if !create { return Ok(None); }
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let mut bytes = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let pass: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    write_private(&path, &pass).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    Ok(Some(pass))
}

fn write_private(path: &Path, data: &str) -> std::io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(data.as_bytes())
}

fn open_client(dir: &Path) -> Result<Client, String> {
    if !Client::exists(dir) {
        return Err(format!("no bot account in {} yet. Run `megatron-bot join <invite-link>` first.", dir.display()));
    }
    let pass = passphrase(dir, false)?.ok_or_else(|| format!("{} has no usable passphrase file", dir.join("passphrase").display()))?;
    Client::open_with(dir, &pass, |relay| HttpTransport::new(relay).into_arc()).map_err(|e| format!("{e:#}"))
}

fn usage() -> ! {
    eprintln!("megatron-bot: a local-model member for Megatron\n\n  megatron-bot join <invite-link>   create the bot's account and join a community\
        \n  megatron-bot run                  answer mentions in every room it is in (the systemd service runs this)\n\n\
        Environment: MEGATRON_BOT_DATA, MEGATRON_BOT_NAME (default {DEFAULT_NAME}), MEGATRON_BOT_MODEL,\
        \n  MEGATRON_BOT_OLLAMA_URL (default {DEFAULT_OLLAMA}), MEGATRON_BOT_DEEPSEEK_URL, MEGATRON_BOT_DEEPSEEK_MODEL,\
        \n  MEGATRON_BOT_DEEPSEEK_KEY_FILE (default ~/.config/deepseek-key), MEGATRON_BOT_SEARCH_URL (default {DEFAULT_SEARCH_URL}),\
        \n  MEGATRON_BOT_STUDIO_URL (default http://127.0.0.1:4576), MEGATRON_BOT_COMFY_URL (default http://127.0.0.1:8188),\
        \n  MEGATRON_BOT_STUDIO_LOCK, MEGATRON_BOT_STUDIO_TOKEN_FILE (default ~/.config/homebase/token),\
        \n  MEGATRON_BOT_STUDIO_DAILY_CAP (default: no limit), MEGATRON_BOT_STUDIO_DAILY_VIDEO_CAP (default: no limit),\
        \n  MEGATRON_BOT_STUDIO_QUEUE_MAX (default 3), MEGATRON_BOT_VOICE_SETTINGS,\
        \n  MEGATRON_BOT_WALL_PERCENT (default 3), MEGATRON_BOT_WALL_DAILY (default 2 per room),\
        \n  MEGATRON_BOT_MAX_PER_MINUTE, MEGATRON_BOT_MAX_REPLY_CHARS");
    std::process::exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("join") => match args.get(1) { Some(link) => join(link), None => { eprintln!("Which invite link? Usage: megatron-bot join <invite-link>"); std::process::exit(2) } },
        Some("run") => run(),
        Some("help" | "-h" | "--help") => usage(),
        _ => usage(),
    };
    if let Err(e) = r { eprintln!("megatron-bot: {e}"); std::process::exit(1); }
}

fn join(link: &str) -> Result<(), String> {
    let dir = data_dir();
    let cfg = config();
    let inv = InviteLink::decode(link).map_err(|e| format!("That is not a Megatron community invite link ({e})."))?;
    let exists = Client::exists(&dir);
    let pass = passphrase(&dir, !exists)?.ok_or_else(|| format!("{} has no usable passphrase file; the account there cannot be unlocked", dir.join("passphrase").display()))?;
    let mut c = if exists {
        println!("Using the bot account already in {}.", dir.display());
        open_client(&dir)?
    } else {
        let t = HttpTransport::new(&inv.relay).into_arc();
        Api { t: t.clone() }.health().map_err(|e| commons_core::api::unreachable_text(&inv.relay, &format!("{e:#}")))?;
        println!("Creating the bot's account \"{}\" on {}.", cfg.name, inv.relay);
        Client::create_with_invite(&dir, &pass, &inv.relay, &cfg.name, &inv.secret, KdfParams::STRONG, t).map_err(|e| format!("could not create the account: {e:#}"))?
    };
    if c.st.communities.contains_key(&inv.community) {
        println!("The bot is already in \"{}\".", inv.name);
        return Ok(());
    }
    let cid = c.join_with_link(link).map_err(|e| format!("could not ask to join: {e:#}"))?;
    c.save().map_err(|e| format!("could not save: {e:#}"))?;
    println!("Asked to join \"{}\". Someone who administers that community has to let the bot in.", inv.name);

    let wait = env_usize("MEGATRON_BOT_JOIN_WAIT_SECS", 90);
    let mut joined = false;
    for i in 0..wait {
        let _ = c.tick(0);
        std::thread::sleep(Duration::from_secs(1));
        if c.st.communities.get(&cid).is_some_and(|co| !co.removed) {
            joined = true;
            break;
        }
        if let Some(refused) = c.st.join_refused.get(&cid) {
            println!("The join was refused: {}", refused.reason);
            return Ok(());
        }
        if i > 0 && i % 15 == 0 { println!("Still waiting for an admin to let the bot in…"); }
    }
    if joined {
        println!("Joined. To keep it answering, start the service: packaging/megatron-bot/enable.sh, or `megatron-bot run`.");
    } else {
        println!("Not in yet. The request is saved; the next `megatron-bot run` (or the service) will keep trying and finish it.");
    }
    Ok(())
}

fn run() -> Result<(), String> {
    let dir = data_dir();
    let cfg = config();
    let mut c = open_client(&dir)?;
    if !c.st.typing_enabled {
        c.st.typing_enabled = true;
        let _ = c.save();
    }

    let timeout = Duration::from_secs(env_usize("MEGATRON_BOT_LLM_TIMEOUT_SECS", 120).max(5) as u64);
    let ollama_url = env_string("MEGATRON_BOT_OLLAMA_URL").or_else(|| env_string("OLLAMA_URL")).unwrap_or_else(|| DEFAULT_OLLAMA.into());
    let preferred = env_string("MEGATRON_BOT_MODEL");
    let local = Ollama::new(&ollama_url, preferred.as_deref(), timeout);
    match local.model() {
        Ok(Some(m)) => eprintln!("[bot] local model: {m} ({ollama_url})"),
        Ok(None) => eprintln!("[bot] Ollama at {ollama_url} has no chat model installed yet"),
        Err(e) => eprintln!("[bot] Ollama at {ollama_url} is not answering yet: {e}"),
    }
    let ds_url = env_string("MEGATRON_BOT_DEEPSEEK_URL").unwrap_or_else(|| DEFAULT_DEEPSEEK_URL.into());
    let ds_model = env_string("MEGATRON_BOT_DEEPSEEK_MODEL").unwrap_or_else(|| DEFAULT_DEEPSEEK_MODEL.into());
    let deepseek = DeepSeek::new(&ds_url, deepseek_key_file(), &ds_model, timeout);
    let search_url = env_string("MEGATRON_BOT_SEARCH_URL").unwrap_or_else(|| DEFAULT_SEARCH_URL.into());
    // MEGATRON_BOT_SEARXNG_URL (e.g. http://127.0.0.1:8092): a local SearXNG instance instead of DuckDuckGo's HTML page.
    let searcher: Arc<dyn WebSearch> = match env_string("MEGATRON_BOT_SEARXNG_URL") {
        Some(sx) => { eprintln!("[bot] web search: SearXNG at {sx}"); Arc::new(SearXng::new(&sx, SEARCH_TIMEOUT)) }
        None => Arc::new(DuckDuckGo::new(&search_url, SEARCH_TIMEOUT)),
    };
    let studio_url = env_string("MEGATRON_BOT_STUDIO_URL").unwrap_or_else(|| DEFAULT_STUDIO_URL.into());
    let studio: Arc<dyn StudioApi> = Arc::new(HttpStudio::new(&studio_url, &studio_token_file()));
    let gate = Arc::new(GpuGate::from_env());

    let bot = Arc::new(Bot::new(
        cfg.clone(),
        Box::new(local),
        Box::new(deepseek),
        searcher,
        studio,
        gate,
        dir.join("bot-state.json"),
    ));
    eprintln!("[bot] running as \"{}\"; answers stay local unless an admin switches a room to DeepSeek", cfg.name);
    serve(c, bot);
    Ok(())
}

fn serve(client: Client, bot: Arc<Bot>) {
    let shared = Arc::new(Mutex::new(client));

    // Relay rounds, decryption, roster changes: continuously, never while a model is being asked.
    {
        let shared = shared.clone();
        std::thread::spawn(move || loop {
            {
                let mut c = shared.lock().unwrap_or_else(|p| p.into_inner());
                let _ = c.tick(0);
            }
            std::thread::sleep(Duration::from_secs(1));
        });
    }

    // GPU manners: every ten seconds, cheaply, ask whether a game or Studio owns the card. If so, free the bot's model
    // at once (Ollama would otherwise hold it for five minutes) and keep it free while the pause lasts. Questions that
    // arrive meanwhile get a short notice in `Bot::answer` instead of loading the model.
    {
        let bot = bot.clone();
        std::thread::spawn(move || {
            let mut last: Option<Pause> = None;
            loop {
                let now = bot.tick_gpu();
                if now != last {
                    match &now {
                        Some(pause) => eprintln!("[bot] GPU needed by {}; local model unloaded", pause.reason()),
                        None => eprintln!("[bot] GPU free again; the local model may load on the next question"),
                    }
                    last = now;
                }
                std::thread::sleep(Duration::from_secs(10));
            }
        });
    }

    // One worker per room: one answer at a time there, with a small queue behind it, and rooms do not wait for each other.
    let mut rooms: HashMap<String, mpsc::SyncSender<Job>> = HashMap::new();
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let jobs = {
            let mut c = shared.lock().unwrap_or_else(|p| p.into_inner());
            bot.poll(&mut c)
        };
        for job in jobs {
            // Unasked: on its own thread, so a song being made for fun never holds up an answer to a real question.
            if matches!(job.kind, JobKind::Chime | JobKind::Wall) {
                let bot = bot.clone();
                let shared = shared.clone();
                std::thread::spawn(move || {
                    let mut chat = SharedClient(&shared);
                    if let Some(sent) = bot.answer(&mut chat, &job) {
                        eprintln!("[bot] joined in, in {} ({} chars)", job.gid, sent.text.chars().count());
                    }
                });
                continue;
            }
            let cap = bot.cfg.max_queue;
            let gid = job.gid.clone();
            let tx = rooms.entry(gid.clone()).or_insert_with(|| {
                let (tx, rx) = mpsc::sync_channel::<Job>(cap);
                let bot = bot.clone();
                let shared = shared.clone();
                std::thread::spawn(move || {
                    while let Ok(job) = rx.recv() {
                        let mut chat = SharedClient(&shared);
                        if let Some(sent) = bot.answer(&mut chat, &job) {
                            eprintln!("[bot] answered in {} ({} chars{})", job.gid, sent.text.chars().count(), if sent.deepseek { ", DeepSeek" } else { "" });
                        }
                    }
                });
                tx
            });
            if tx.try_send(job).is_err() {
                eprintln!("[bot] queue full for room {gid}; dropped a request");
            }
        }
    }
}
