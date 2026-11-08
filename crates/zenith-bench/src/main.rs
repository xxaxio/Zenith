//! Zenith Bench: Multi-scenario high-throughput benchmarking, botnet simulations, and memory profiling CLI.

mod affinity;
mod metrics;
mod scenarios;

use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;
use tracing::info;

const USAGE: &str = "\
Usage: zenith-bench <subcommand> [OPTIONS]

Subcommands:
  overhead        [target] [players] [rate] [secs] [concurrency]
                  Differential A/B benchmark: measures baseline direct echo latency
                  vs Zenith proxied latency and computes added microsecond overhead (P50/P95/P99).
                  Example: zenith-bench overhead 127.0.0.1:25565 100 20 10 100

  limbo-ultimate  [target] [count] [secs] [concurrency]
                  High-fidelity real-condition benchmark: 100% protocol 777 bots ticking at 20 Hz,
                  human jitter, /ping commands, live telemetry, and percentiles (p50/p90/p99/Max).
                  Example: zenith-bench limbo-ultimate 127.0.0.1:25565 1000 20 200

  limbo-holding   [target] [count] [secs] [concurrency]
                  Holds N bots in virtual limbo void; measures RAM RSS (< 1 KiB/sess) and KeepAlive latency.
                  Example: zenith-bench limbo-holding 127.0.0.1:25565 5000 15 500

  queue-drain     [target] [backend] [count] [concurrency]
                  Spawns mock Horizon backend, loads priority queue with VIPs & standard bots, and measures drain throughput.
                  Example: zenith-bench queue-drain 127.0.0.1:25565 127.0.0.1:25566 2000 200

  adversary-storm [target] [attackers] [proxy_file] [concurrency]
                  Executes realistic cyber attacks (Slowloris, Jumbo frames, Malformed VarInt, Token flood, and multi-proxy rotation).
                  Example: zenith-bench adversary-storm 127.0.0.1:25565 500 C:\\path\\to\\proxies.txt 100

  kernel-siege    [target] [attackers] [legit_count] [secs] [concurrency]
                  Hardcore siege: multi-vector bot swarms (buffer overflow, blocked brand, flood)
                  blasting Zenith while a canary cohort of real players plays in Limbo.
                  Verifies 0% false positives, kernel escalation, and rock-solid latency.
                  Example: zenith-bench kernel-siege 127.0.0.1:25565 500 20 10 150

  realistic-mix   [target] [total] [secs] [concurrency]
                  Production mix: 70% Vanilla + 20% Horizon Turbo + 10% Adversaries/Noise.
                  Example: zenith-bench realistic-mix 127.0.0.1:25565 2000 15 300

  phase7          [target] [count] [secs] [concurrency]
                  Phase 7 ingress: PROXY v2 binary stream, resource pack pre-caching, and broadcast alerts.
                  Example: zenith-bench phase7 127.0.0.1:25565 1000 15 300

Flags:
  --pin-cores     [SPEC] Pin benchmark process to logical cores (e.g. 'p-cores', 'e-cores', '0-3')
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

    // Check for optional --pin-cores flag
    if let Some(pos) = args.iter().position(|a| a == "--pin-cores") {
        if let Some(spec) = args.get(pos + 1) {
            match affinity::parse_affinity_mask(spec) {
                Ok(mask) => {
                    if let Err(e) = affinity::apply_process_affinity(mask) {
                        tracing::warn!("Failed to apply CPU affinity: {}", e);
                    } else {
                        info!("CPU affinity pinned to mask 0x{:X} ({})", mask, spec);
                    }
                }
                Err(e) => tracing::error!("Invalid --pin-cores specifier: {}", e),
            }
        }
    }

    let subcommand = args.get(1).map(|s| s.as_str()).unwrap_or("--help");

    match subcommand {
        "overhead" | "ab" => {
            let target = args
                .get(2)
                .cloned()
                .unwrap_or_else(|| "127.0.0.1:25565".to_string());
            let players: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(100);
            let rate: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(20);
            let secs: u64 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(10);
            let concurrency: usize = args.get(6).and_then(|s| s.parse().ok()).unwrap_or(players);

            scenarios::run_overhead_ab(&target, players, rate, secs, concurrency).await?;
        }
        "limbo-ultimate" => {
            let target = args
                .get(2)
                .cloned()
                .unwrap_or_else(|| "127.0.0.1:25565".to_string());
            let count: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1000);
            let secs: u64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(20);
            let concurrency: usize = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(count);
            let (host, port) = parse_host_port(&target);
            let metrics_url = "127.0.0.1:25580";

            scenarios::run_limbo_ultimate(
                &host,
                port,
                metrics_url,
                count,
                Duration::from_secs(secs),
                concurrency,
                Duration::from_millis(5),
            )
            .await?;
        }
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
        "kernel-siege" | "siege" => {
            let target = args
                .get(2)
                .cloned()
                .unwrap_or_else(|| "127.0.0.1:25565".to_string());
            let attackers: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(500);
            let legit: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(20);
            let secs: u64 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(10);
            let concurrency: usize = args.get(6).and_then(|s| s.parse().ok()).unwrap_or(150);

            let (host, port) = parse_host_port(&target);
            let metrics_url = "127.0.0.1:25580";

            scenarios::run_kernel_siege(
                &host,
                port,
                metrics_url,
                attackers,
                legit,
                Duration::from_secs(secs),
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
        "phase7" | "phase7-ingress" => {
            let target = args
                .get(2)
                .cloned()
                .unwrap_or_else(|| "127.0.0.1:25565".to_string());
            let count: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1000);
            let secs: u64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(15);
            let concurrency: usize = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(300);
            let (host, port) = parse_host_port(&target);
            let metrics_url = "127.0.0.1:25580";

            scenarios::run_phase7_bench(
                &host,
                port,
                metrics_url,
                count,
                Duration::from_secs(secs),
                concurrency,
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
