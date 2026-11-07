//! Zenith Bench: Multi-scenario high-throughput benchmarking, botnet simulations, and memory profiling CLI.

mod metrics;
mod scenarios;

use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

const USAGE: &str = "\
Usage: zenith-bench <subcommand> [OPTIONS]

Subcommands:
  limbo-holding   [target] [count] [secs] [concurrency]
                  Holds N bots in virtual limbo void; measures RAM RSS (< 1 KiB/sess) and KeepAlive latency.
                  Example: zenith-bench limbo-holding 127.0.0.1:25565 5000 15 500

  queue-drain     [target] [backend] [count] [concurrency]
                  Spawns mock Horizon backend, loads priority queue with VIPs & standard bots, and measures drain throughput.
                  Example: zenith-bench queue-drain 127.0.0.1:25565 127.0.0.1:25566 2000 200

  adversary-storm [target] [attackers] [proxy_file] [concurrency]
                  Executes realistic cyber attacks (Slowloris, Jumbo frames, Malformed VarInt, Token flood, and multi-proxy rotation).
                  Example: zenith-bench adversary-storm 127.0.0.1:25565 500 C:\\path\\to\\proxies.txt 100

  realistic-mix   [target] [total] [secs] [concurrency]
                  Production mix: 70% Vanilla + 20% Horizon Turbo + 10% Adversaries/Noise.
                  Example: zenith-bench realistic-mix 127.0.0.1:25565 2000 15 300

Flags:
  -h, --help      Display this help message
";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = env::args().collect();
    let subcommand = args.get(1).map(|s| s.as_str()).unwrap_or("--help");

    match subcommand {
        "limbo-holding" => {
            let target = args
                .get(2)
                .cloned()
                .unwrap_or_else(|| "127.0.0.1:25565".to_string());
            let count: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(2000);
            let secs: u64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(15);
            let concurrency: usize = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(300);
            let (host, port) = parse_host_port(&target);
            let metrics_url = "127.0.0.1:25580";

            scenarios::run_limbo_holding(
                &host,
                port,
                metrics_url,
                count,
                Duration::from_secs(secs),
                concurrency,
            )
            .await?;
        }
        "queue-drain" => {
            let target = args
                .get(2)
                .cloned()
                .unwrap_or_else(|| "127.0.0.1:25565".to_string());
            let backend_str = args
                .get(3)
                .cloned()
                .unwrap_or_else(|| "127.0.0.1:25566".to_string());
            let count: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(1000);
            let concurrency: usize = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(200);

            let (host, port) = parse_host_port(&target);
            let backend_addr: SocketAddr = backend_str.parse()?;

            scenarios::run_queue_drain(&host, port, backend_addr, count, concurrency).await?;
        }
        "adversary-storm" => {
            let target = args
                .get(2)
                .cloned()
                .unwrap_or_else(|| "127.0.0.1:25565".to_string());
            let count: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(300);
            let proxy_path = args.get(4).map(PathBuf::from);
            let concurrency: usize = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(100);

            let (host, port) = parse_host_port(&target);
            let metrics_url = "127.0.0.1:25580";

            scenarios::run_adversary_storm(
                &host,
                port,
                metrics_url,
                count,
                proxy_path.as_deref(),
                concurrency,
            )
            .await?;
        }
        "realistic-mix" => {
            let target = args
                .get(2)
                .cloned()
                .unwrap_or_else(|| "127.0.0.1:25565".to_string());
            let count: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1000);
            let secs: u64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(15);
            let concurrency: usize = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(300);

            let (host, port) = parse_host_port(&target);

            scenarios::run_realistic_mix(
                &host,
                port,
                scenarios::RealisticMixConfig {
                    total_bots: count,
                    vanilla_pct: 70,
                    horizon_pct: 20,
                    adversary_pct: 10,
                    session_duration: Duration::from_secs(secs),
                    concurrency_limit: concurrency,
                },
            )
            .await?;
        }
        _ => {
            println!("{USAGE}");
        }
    }

    Ok(())
}

fn parse_host_port(s: &str) -> (String, u16) {
    if let Ok(addr) = s.parse::<SocketAddr>() {
        (addr.ip().to_string(), addr.port())
    } else {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() == 2 {
            (parts[0].to_string(), parts[1].parse().unwrap_or(25565))
        } else {
            (s.to_string(), 25565)
        }
    }
}
