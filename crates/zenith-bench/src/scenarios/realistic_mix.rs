//! Scenario 4: Realistic Production Mix - Vanilla, Horizon Turbo, and background noise/adversaries.

use crate::metrics::{LatencyHistogram, Live, print_summary_table};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tracing::info;
use zenith_testkit::{AdversaryStrategy, ClientOutcome, ClientType, SyntheticClient};

/// Configuration options for the realistic production mix scenario.
#[derive(Debug, Clone, Copy)]
pub struct RealisticMixConfig {
    pub total_bots: usize,
    pub vanilla_pct: usize,
    pub horizon_pct: usize,
    pub adversary_pct: usize,
    pub session_duration: Duration,
    pub concurrency_limit: usize,
}

pub async fn run_realistic_mix(
    target_host: &str,
    target_port: u16,
    config: RealisticMixConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    info!(
        "Starting Realistic Mix scenario: {} bots ({}% Vanilla, {}% Horizon, {}% Adversary) -> {}:{}",
        config.total_bots,
        config.vanilla_pct,
        config.horizon_pct,
        config.adversary_pct,
        target_host,
        target_port
    );

    let live = Live::new();
    let latencies = LatencyHistogram::new();
    let semaphore = Arc::new(Semaphore::new(config.concurrency_limit));
    let start_time = Instant::now();

    let mut handles = Vec::with_capacity(config.total_bots);

    for i in 0..config.total_bots {
        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let live_cloned = live.clone();
        let host = target_host.to_string();
        let port = target_port;
        let lat_cloned = latencies.clone();

        let bucket = i % 100;
        let client_type = if bucket < config.vanilla_pct {
            ClientType::Vanilla {
                ping_jitter: Duration::from_millis(40 + (i % 60) as u64),
            }
        } else if bucket < config.vanilla_pct + config.horizon_pct {
            ClientType::Horizon
        } else {
            ClientType::Adversary(AdversaryStrategy::TokenBucketFlood { count: 80 })
        };

        let username = format!("MixClient_{i:04}");
        let session_duration = config.session_duration;

        let handle = tokio::spawn(async move {
            let _permit = permit;
            let client = SyntheticClient::new(&username, client_type);

            live_cloned
                .connected
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            let run_start = Instant::now();
            match client.run(&host, port, session_duration).await {
                Ok(ClientOutcome::HeldInLimbo {
                    keepalives_answered,
                    ..
                }) => {
                    lat_cloned.record(run_start.elapsed()).await;
                    live_cloned
                        .joined_limbo
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    live_cloned
                        .keepalives_answered
                        .fetch_add(keepalives_answered, std::sync::atomic::Ordering::Relaxed);
                }
                Ok(ClientOutcome::Transferred { .. }) => {
                    live_cloned
                        .transferred
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                Ok(ClientOutcome::ShieldJailed) => {
                    live_cloned
                        .jailed_or_blocked
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                Ok(_) => {}
                Err(_) => {
                    live_cloned
                        .errors
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
        });

        handles.push(handle);
        if i % 100 == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    // Wait for all bots
    for h in handles {
        let _ = h.await;
    }

    let elapsed = start_time.elapsed();
    let snapshot = live.snapshot();
    let pcts = latencies.calculate_percentiles().await;

    print_summary_table(
        "ZENITH BENCH: SCENARIO 4 - REALISTIC PRODUCTION MIX REPORT",
        &[
            ("Total Bots Injected", config.total_bots.to_string()),
            (
                "Legitimate Limbo Sessions",
                snapshot.joined_limbo.to_string(),
            ),
            ("Transfers Completed", snapshot.transferred.to_string()),
            (
                "Adversaries Filtered / Jailed",
                snapshot.jailed_or_blocked.to_string(),
            ),
            (
                "KeepAlives Answered",
                snapshot.keepalives_answered.to_string(),
            ),
            ("Network / IO Errors", snapshot.errors.to_string()),
            ("KeepAlive Latency p50", format!("{:.2?}", pcts.p50)),
            ("KeepAlive Latency p90", format!("{:.2?}", pcts.p90)),
            ("KeepAlive Latency p99", format!("{:.2?}", pcts.p99)),
            ("KeepAlive Latency Max", format!("{:.2?}", pcts.max)),
            ("Total Simulation Duration", format!("{elapsed:.2?}")),
        ],
    );

    Ok(())
}
