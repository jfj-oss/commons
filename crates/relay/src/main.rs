use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter(std::env::var("RUST_LOG").unwrap_or("info".into())).init();
    let mut bind: SocketAddr = "127.0.0.1:8787".parse().unwrap();
    let mut data = std::path::PathBuf::from("./relay-data");
    // Call support (optional). The API secret comes from the environment or a file, never from argv.
    let mut lk_url = std::env::var("COMMONS_LIVEKIT_URL").ok().filter(|x| !x.is_empty());
    let mut lk_key = std::env::var("COMMONS_LIVEKIT_API_KEY").ok().filter(|x| !x.is_empty());
    let mut lk_secret = std::env::var("COMMONS_LIVEKIT_API_SECRET").ok().filter(|x| !x.is_empty());
    let lk_ttl: u64 = std::env::var("COMMONS_LIVEKIT_TOKEN_TTL").ok().and_then(|x| x.parse().ok()).unwrap_or(900);
    let mut a = std::env::args().skip(1);
    while let Some(x) = a.next() {
        match x.as_str() {
            "--bind" => bind = a.next().expect("--bind addr").parse().expect("addr"),
            "--data" => data = a.next().expect("--data dir").into(),
            "--livekit-url" => lk_url = Some(a.next().expect("--livekit-url ws(s)://host")),
            "--livekit-key" => lk_key = Some(a.next().expect("--livekit-key api-key")),
            "--livekit-secret-file" => lk_secret = Some(std::fs::read_to_string(a.next().expect("--livekit-secret-file path")).expect("read secret file").trim().to_string()),
            _ => { eprintln!("usage: commons-relay [--bind 127.0.0.1:8787] [--data ./relay-data] [--livekit-url wss://host --livekit-key K --livekit-secret-file F]"); std::process::exit(2) }
        }
    }
    if !bind.ip().is_loopback() {
        eprintln!("warning: binding to non-loopback address {bind}. Run behind a TLS-terminating proxy; see docs/DEPLOY.md");
    }
    let lk = match (lk_url, lk_key, lk_secret) {
        (Some(url), Some(api_key), Some(api_secret)) => {
            if api_secret.len() < 32 { eprintln!("refusing to start: the LiveKit API secret must be at least 32 characters"); std::process::exit(2) }
            tracing::info!("call tokens enabled for {url}");
            Some(commons_relay::LiveKitCfg { api_key, api_secret, url, token_ttl_secs: lk_ttl })
        }
        (None, None, None) => None,
        _ => { eprintln!("call support needs all of: livekit url, key and secret"); std::process::exit(2) }
    };
    let h = commons_relay::start_with(bind, data, lk).await.expect("start relay");
    tracing::info!("commons relay listening on {}", h.addr);
    let _ = tokio::signal::ctrl_c().await;
    h.stop().await;
}
