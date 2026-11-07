//! Scenario 5: Phase 7 Benchmark - PROXY Protocol v2, Resource Pack Preload & Broadcast Latency.

use crate::metrics::{
    LatencyHistogram, Live, fetch_zenith_prometheus_metrics, print_summary_table,
};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tracing::info;
use zenith_testkit::{ClientOutcome, ClientType, SyntheticClient};
use zenith_wire::{ProxyCommand, ProxyHeader};

pub async fn run_phase7_bench(
    target_host: &str,
    target_port: u16,
    metrics_url: &str,
    bot_count: usize,
    hold_duration: Duration,
    concurrency_limit: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    info!(
        "Starting Phase 7 Benchmark: {} bots (PROXY v2 + Resource Preload + Broadcasts) -> {}:{}",
        bot_count, target_host, target_port
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
            let username = format!("ProxyBot_{i:05}");

            // Generate unique virtual client IP and subnet behind upstream proxy
            let o1 = (11 + (i % 200)) as u8;
            let o2 = ((i / 200) % 250) as u8;
            let o3 = ((i / 50) % 250) as u8;
            let o4 = ((i % 250) + 1) as u8;
            let src_ip = Ipv4Addr::new(o1, o2, o3, o4);
            let src_port = 10000 + (i % 50000) as u16;
            let dst_ip = Ipv4Addr::new(127, 0, 0, 1);
            let dst_port = port;

            let proxy_hdr = ProxyHeader {
                command: ProxyCommand::Proxy,
                src_addr: Some(SocketAddr::V4(SocketAddrV4::new(src_ip, src_port))),
                dst_addr: Some(SocketAddr::V4(SocketAddrV4::new(dst_ip, dst_port))),
            };

            live_cloned
                .proxy_headers_sent
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            let client = SyntheticClient::new(
                &username,
                ClientType::Vanilla {
                    ping_jitter: Duration::from_millis(20),
                },
            )
            .with_proxy_v2(proxy_hdr);

            live_cloned
                .connected
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            let run_start = Instant::now();
            match client.run(&host, port, hold_duration).await {
                Ok(ClientOutcome::HeldInLimbo {
                    keepalives_answered,
                    resource_packs_loaded,
                    alerts_received,
                }) => {
                    let elapsed = run_start.elapsed();
                    lat_cloned.record(elapsed).await;
                    live_cloned
                        .joined_limbo
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    live_cloned
                        .keepalives_answered
                        .fetch_add(keepalives_answered, std::sync::atomic::Ordering::Relaxed);
                    live_cloned
                        .resource_packs_loaded
                        .fetch_add(resource_packs_loaded, std::sync::atomic::Ordering::Relaxed);
                    live_cloned
                        .alerts_received
                        .fetch_add(alerts_received, std::sync::atomic::Ordering::Relaxed);
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

        if i % 100 == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
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
        "ZENITH BENCH: PHASE 7 - PROXY V2, RESOURCE PRELOAD & ALERT SCENARIO",
        &[
            ("Total Bots Targeted", bot_count.to_string()),
            (
                "PROXY Protocol v2 Headers Sent",
                snapshot.proxy_headers_sent.to_string(),
            ),
            ("Held In Virtual Limbo", snapshot.joined_limbo.to_string()),
            (
                "Resource Packs Preloaded (Turnaround)",
                snapshot.resource_packs_loaded.to_string(),
            ),
            (
                "KeepAlives Successfully Answered",
                snapshot.keepalives_answered.to_string(),
            ),
            (
                "Broadcast & Alert Packets Received",
                snapshot.alerts_received.to_string(),
            ),
            (
                "Shield Jailed / Drops",
                snapshot.jailed_or_blocked.to_string(),
            ),
            ("Connection Errors / Drops", snapshot.errors.to_string()),
            ("Elapsed Scenario Duration", format!("{elapsed:.2?}")),
            ("Connect Latency P50", format!("{:.2?}", pcts.p50)),
            ("Connect Latency P90", format!("{:.2?}", pcts.p90)),
            ("Connect Latency P99", format!("{:.2?}", pcts.p99)),
            ("Measured Memory RSS", rss_str),
            ("Memory RSS Invariant", per_session_str),
        ],
    );

    Ok(())
}
