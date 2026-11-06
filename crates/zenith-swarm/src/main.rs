//! Zenith Swarm: Synthetic bot load generator and benchmark harness.

use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    info!("Zenith Swarm load harness ready.");
    Ok(())
}
