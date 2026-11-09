use std::net::SocketAddr;

const USAGE: &str = "usage: commons-relay [--bind 127.0.0.1:8787] [--data ./relay-data] [--max-accounts N] [--quota-mb N] [--open]
  COMMONS_RELAY_INVITE=<secret>  new accounts need this secret to register (set it in the environment, not on the command line)
  --open                         allow a non-loopback bind with no invite secret (anyone who can reach it can register)
  --quota-mb N                   per-account limit for messages and, separately, for files
  --large-quota-gb N             large-file storage per member account (default 10)
  --large-owner-quota-gb N       the same for a community's owner (default 100)
  --large-total-gb N             most large-file storage the whole relay holds (default 250)
  --large-expire-days N          days a large file stays before it is removed (default 7)
  --site DIR                     serve the /join page, the files in DIR from /download/, and app updates from DIR/update/
  --min-app-version X.Y.Z        tell apps older than this that they must update (or COMMONS_MIN_APP_VERSION; docs/UPDATES.md)
  --front ADDR                   one port for everything behind a TLS terminator (docs/DEPLOY.md); needs an invite secret
  --front-livekit ADDR           where /rtc goes from --front
  --front-turn ADDR              where TURN goes from --front
  --turn-public-port N           the port clients reach TURN over TLS on, if not 443\n  calls (optional): --livekit-url wss://host --livekit-key K --livekit-secret-file F  (or COMMONS_LIVEKIT_URL / _API_KEY / _API_SECRET)";

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter(std::env::var("RUST_LOG").unwrap_or("info".into())).init();
    let mut bind: SocketAddr = "127.0.0.1:8787".parse().unwrap();
    let mut data = std::path::PathBuf::from("./relay-data");
    let mut limits = commons_relay::Limits { invite_secret: std::env::var("COMMONS_RELAY_INVITE").ok().filter(|s| !s.is_empty()), ..Default::default() };
    // Call support (optional). The API secret comes from the environment or a file, never from argv.
    let mut lk_url = std::env::var("COMMONS_LIVEKIT_URL").ok().filter(|x| !x.is_empty());
    let mut lk_key = std::env::var("COMMONS_LIVEKIT_API_KEY").ok().filter(|x| !x.is_empty());
    let mut lk_secret = std::env::var("COMMONS_LIVEKIT_API_SECRET").ok().filter(|x| !x.is_empty());
    let lk_ttl: u64 = std::env::var("COMMONS_LIVEKIT_TOKEN_TTL").ok().and_then(|x| x.parse().ok()).unwrap_or(900);
    limits.min_app_version = std::env::var("COMMONS_MIN_APP_VERSION").ok().filter(|x| !x.is_empty());
    let mut open = false;
    let (mut front, mut front_lk, mut front_turn, mut turn_port) = (None::<SocketAddr>, None::<SocketAddr>, None::<SocketAddr>, std::env::var("COMMONS_TURN_PUBLIC_PORT").ok().and_then(|x| x.parse::<u16>().ok()));
    let mut a = std::env::args().skip(1);
    while let Some(x) = a.next() {
        match x.as_str() {
            "--bind" => bind = a.next().expect("--bind addr").parse().expect("addr"),
            "--data" => data = a.next().expect("--data dir").into(),
            "--open" => open = true,
            "--site" => limits.site_dir = Some(a.next().expect("--site dir").into()),
            "--min-app-version" => limits.min_app_version = Some(a.next().expect("--min-app-version X.Y.Z")),
            "--front" => front = Some(a.next().expect("--front addr").parse().expect("addr")),
            "--front-livekit" => front_lk = Some(a.next().expect("--front-livekit addr").parse().expect("addr")),
            "--front-turn" => front_turn = Some(a.next().expect("--front-turn addr").parse().expect("addr")),
            "--turn-public-port" => turn_port = Some(a.next().expect("--turn-public-port N").parse().expect("port")),
            "--max-accounts" => limits.max_accounts = a.next().expect("--max-accounts N").parse().expect("number"),
            "--quota-mb" => {
                let mb: u64 = a.next().expect("--quota-mb N").parse().expect("number");
                limits.max_entry_bytes_per_account = mb << 20;
                limits.max_blob_bytes_per_account = mb << 20;
            }
            "--large-quota-gb" => limits.large_file_quota_per_account = a.next().expect("--large-quota-gb N").parse::<u64>().expect("number") << 30,
            "--large-owner-quota-gb" => limits.large_file_quota_owner = a.next().expect("--large-owner-quota-gb N").parse::<u64>().expect("number") << 30,
            "--large-total-gb" => limits.large_file_total = a.next().expect("--large-total-gb N").parse::<u64>().expect("number") << 30,
            "--large-expire-days" => limits.large_file_expire_secs = a.next().expect("--large-expire-days N").parse::<u64>().expect("number") * 86400,
            "--livekit-url" => lk_url = Some(a.next().expect("--livekit-url ws(s)://host")),
            "--livekit-key" => lk_key = Some(a.next().expect("--livekit-key api-key")),
            "--livekit-secret-file" => lk_secret = Some(std::fs::read_to_string(a.next().expect("--livekit-secret-file path")).expect("read secret file").trim().to_string()),
            _ => { eprintln!("{USAGE}"); std::process::exit(2) }
        }
    }
    if let Some(v) = &limits.min_app_version {
        if !commons_relay::is_app_version(v) { eprintln!("refusing to start: the minimum app version {v:?} is not a version like 0.2.0"); std::process::exit(2) }
        tracing::info!("apps older than {v} are told to update");
    }
    if !bind.ip().is_loopback() {
        eprintln!("warning: binding to non-loopback address {bind}. Run behind a TLS-terminating proxy; see docs/DEPLOY.md");
        if limits.invite_secret.is_none() {
            if !open { eprintln!("refusing to start: {bind} is not loopback and COMMONS_RELAY_INVITE is not set, so anyone who can reach it could register. Set the secret (docs/DEPLOY.md), or pass --open to run an open relay on purpose"); std::process::exit(2) }
            eprintln!("warning: --open: anyone who can reach this address can register");
        }
    }
    // The relay itself stays on loopback behind a front, so the check above would not catch an open door.
    if front.is_some() && limits.invite_secret.is_none() { eprintln!("refusing to start: --front means the relay can be reached from outside, and COMMONS_RELAY_INVITE is not set"); std::process::exit(2) }
    limits.livekit = match (lk_url, lk_key, lk_secret) {
        (Some(url), Some(api_key), Some(api_secret)) => {
            if api_secret.starts_with("REPLACE") { eprintln!("refusing to start: the LiveKit API secret is still the REPLACE placeholder from the template"); std::process::exit(2) }
            if api_secret.len() < 32 { eprintln!("refusing to start: the LiveKit API secret must be at least 32 characters"); std::process::exit(2) }
            tracing::info!("call tokens enabled for {url}");
            Some(commons_relay::LiveKitCfg { api_key, api_secret, url, token_ttl_secs: lk_ttl, turn_port, api_url: front_lk.map(|a| format!("http://{a}")) })
        }
        (None, None, None) => None,
        _ => { eprintln!("call support needs all of: livekit url, key and secret"); std::process::exit(2) }
    };
    let h = commons_relay::start_with(bind, data, limits).await.expect("start relay");
    tracing::info!("commons relay listening on {}", h.addr);
    let fronted = match front {
        Some(addr) => {
            let f = commons_relay::front::start_front(addr, commons_relay::front::Front { relay: h.addr, livekit: front_lk, turn: front_turn }).await.expect("start front");
            tracing::info!("front listening on {}", f.addr);
            Some(f)
        }
        None => None,
    };
    // go-offline.sh and service managers stop it with SIGTERM.
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal handler");
    tokio::select! { _ = tokio::signal::ctrl_c() => {} _ = term.recv() => {} }
    if let Some(f) = fronted { f.stop(); }
    h.stop().await;
}
