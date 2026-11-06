use std::net::SocketAddr;

const USAGE: &str = "usage: commons-relay [--bind 127.0.0.1:8787] [--data ./relay-data] [--max-accounts N] [--quota-mb N]
  COMMONS_RELAY_INVITE=<secret>  new accounts need this secret to register (set it in the environment, not on the command line)
  --quota-mb N                   per-account limit for messages and, separately, for files";

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter(std::env::var("RUST_LOG").unwrap_or("info".into())).init();
    let mut bind: SocketAddr = "127.0.0.1:8787".parse().unwrap();
    let mut data = std::path::PathBuf::from("./relay-data");
    let mut limits = commons_relay::Limits { invite_secret: std::env::var("COMMONS_RELAY_INVITE").ok().filter(|s| !s.is_empty()), ..Default::default() };
    let mut a = std::env::args().skip(1);
    while let Some(x) = a.next() {
        match x.as_str() {
            "--bind" => bind = a.next().expect("--bind addr").parse().expect("addr"),
            "--data" => data = a.next().expect("--data dir").into(),
            "--max-accounts" => limits.max_accounts = a.next().expect("--max-accounts N").parse().expect("number"),
            "--quota-mb" => {
                let mb: u64 = a.next().expect("--quota-mb N").parse().expect("number");
                limits.max_entry_bytes_per_account = mb << 20;
                limits.max_blob_bytes_per_account = mb << 20;
            }
            _ => { eprintln!("{USAGE}"); std::process::exit(2) }
        }
    }
    if !bind.ip().is_loopback() {
        eprintln!("warning: binding to non-loopback address {bind}. Run behind a TLS-terminating proxy; see docs/DEPLOY.md");
        if limits.invite_secret.is_none() { eprintln!("warning: anyone who can reach this address can register. Set COMMONS_RELAY_INVITE; see docs/DEPLOY.md"); }
    }
    let h = commons_relay::start_with(bind, data, limits).await.expect("start relay");
    tracing::info!("commons relay listening on {}", h.addr);
    let _ = tokio::signal::ctrl_c().await;
    h.stop().await;
}
