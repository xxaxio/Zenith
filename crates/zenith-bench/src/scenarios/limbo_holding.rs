//! Scenario 1: Limbo Holding - Scalability, idle CPU overhead, and memory RSS validation.

use crate::metrics::{
    LatencyHistogram, Live, fetch_zenith_prometheus_metrics, print_summary_table,
};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tracing::info;
use zenith_testkit::{ClientOutcome, ClientType, SyntheticClient};

pub async fn run_limbo_holding(
    target_host: &str,
    target_port: u16,
    metrics_url: &str,
    bot_count: usize,
    hold_duration: Duration,
    concurrency_limit: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    info!(
        "Starting Limbo Holding scenario: {} bots -> {}:{} for {:?}",
        bot_count, target_host, target_port, hold_duration
    );

    let live = Live::new();
    let latencies = LatencyHistogram::new();
    let semaphore = Arc::new(Semaphore::new(concurrency_limit));
    let start_time = Instant::now();

    let mut handles = Vec::with_capacity(bot_count);

    for i in 0..bot_count {
        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let live_cloned = live.clone();
        let host = target_host.to_string();
        let port = target_port;
        let lat_cloned = latencies.clone();

        let handle = tokio::spawn(async move {
            let _permit = permit;
            let username = format!("LimboBot_{i:05}");
            let client = SyntheticClient::new(
                &username,
                ClientType::Vanilla {
                    ping_jitter: Duration::from_millis(50),
                },
            );

            live_cloned
                .connected
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            let run_start = Instant::now();
            match client.run(&host, port, hold_duration).await {
                Ok(ClientOutcome::HeldInLimbo {
                    keepalives_answered,
                }) => {
                    let elapsed = run_start.elapsed();
                    lat_cloned.record(elapsed).await;
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
                Ok(ClientOutcome::Disconnected) => {
                    // Normal disconnect
                }
                Err(e) => {
                    tracing::warn!("Client failed: {:?}", e);
                    live_cloned
                        .errors
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
        });

        handles.push(handle);

        // Gentle stagger to prevent OS ephemeral port exhaustion
        if i % 100 == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    // Wait for all bots to complete their hold duration
    for h in handles {
        let _ = h.await;
    }

    let elapsed = start_time.elapsed();
    let snapshot = live.snapshot();
    let pcts = latencies.calculate_percentiles().await;
    let prom = fetch_zenith_prometheus_metrics(metrics_url).await;

    let (rss_str, per_session_str) = if let Some(p) = prom {
        let mb = p.memory_rss_bytes as f64 / (1024.0 * 1024.0);
        let per_sess_kib = if p.active_sessions > 0 {
            (p.memory_rss_bytes as f64 / p.active_sessions as f64) / 1024.0
        } else {
            0.0
        };
        (
            format!("{mb:.2} MiB"),
            format!("{per_sess_kib:.2} KiB/session (< 1 KiB target)"),
        )
    } else {
        ("N/A (Prometheus unreached)".to_string(), "N/A".to_string())
    };

    print_summary_table(
        "ZENITH BENCH: SCENARIO 1 - LIMBO HOLDING REPORT",
        &[
            ("Total Bots Targeted", bot_count.to_string()),
            ("Total Held In Limbo", snapshot.joined_limbo.to_string()),
            (
                "KeepAlives Successfully Answered",
                snapshot.keepalives_answered.to_string(),
            ),
            ("Connection Errors / Drops", snapshot.errors.to_string()),
            ("Elapsed Scenario Duration", format!("{elapsed:.2?}")),
            (
                "KeepAlive Round-Trip Avg",
                format!(
                    "{:.2?}",
                    pcts.avg / (snapshot.keepalives_answered.max(1) as u32)
                ),
            ),
            ("Measured Memory RSS", rss_str),
            ("Memory RSS Invariant", per_session_str),
        ],
    );

    Ok(())
}
