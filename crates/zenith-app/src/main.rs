//! Zenith: Ultra-high-density virtual dimension, queue & fallback engine in Rust.

use std::env;
use tokio::net::TcpListener;
use tracing::{error, info};
use zenith_session::VirtualSession;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = env::args().collect();
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("zenith 0.1.0");
        return Ok(());
    }

    if args.iter().any(|arg| arg == "--check") {
        info!("Zenith configuration check passed.");
        return Ok(());
    }

    let bind_addr = env::var("ZENITH_BIND").unwrap_or_else(|_| "127.0.0.1:25565".to_string());
    info!("Starting Zenith v0.1.0 (Protocol 777 / Minecraft 26.3)...");
    info!("Listening for virtual limbo connections on {}", bind_addr);

    let listener = TcpListener::bind(&bind_addr).await?;
    info!("Zenith gatekeeper active. Ready to hold and route players.");

    loop {
        let (socket, peer_addr) = listener.accept().await?;
        info!(peer = %peer_addr, "Accepted new player connection");

        tokio::spawn(async move {
            let mut session = VirtualSession::new(socket);
            if let Err(e) = session.run().await {
                error!(session = ?session.id, error = %e, "Session terminated with error");
            } else {
                info!(session = ?session.id, "Session terminated cleanly");
            }
        });
    }
}
