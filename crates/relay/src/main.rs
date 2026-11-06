use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter(std::env::var("RUST_LOG").unwrap_or("info".into())).init();
    let mut bind: SocketAddr = "127.0.0.1:8787".parse().unwrap();
    let mut data = std::path::PathBuf::from("./relay-data");
    let mut a = std::env::args().skip(1);
    while let Some(x) = a.next() {
        match x.as_str() {
            "--bind" => bind = a.next().expect("--bind addr").parse().expect("addr"),
            "--data" => data = a.next().expect("--data dir").into(),
            _ => { eprintln!("usage: commons-relay [--bind 127.0.0.1:8787] [--data ./relay-data]"); std::process::exit(2) }
        }
    }
    if !bind.ip().is_loopback() {
        eprintln!("warning: binding to non-loopback address {bind}. Run behind a TLS-terminating proxy; see docs/DEPLOY.md");
    }
    let h = commons_relay::start(bind, data).await.expect("start relay");
    tracing::info!("commons relay listening on {}", h.addr);
    let _ = tokio::signal::ctrl_c().await;
    h.stop().await;
}
