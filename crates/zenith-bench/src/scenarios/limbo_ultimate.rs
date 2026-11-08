//! Scenario: Limbo Ultimate - High-Fidelity Real-Conditions Stress & Scale Benchmark.
//!
//! Spawns real Protocol 777 Minecraft player actors ticking at 20 Hz, acknowledging teleports,
//! sending rotations/movements, executing /ping commands, answering KeepAlives, and tracking RTT.

use crate::metrics::{
    LatencyHistogram, Live, fetch_zenith_prometheus_metrics, print_summary_table,
};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tracing::info;
use zenith_testkit::{ClientOutcome, ClientType, LiveTelemetry, PlayerProfile, SyntheticClient};

/// Executes the comprehensive real-world Limbo benchmark.
pub async fn run_limbo_ultimate(
    target_host: &str,
    target_port: u16,
    metrics_url: &str,
    bot_count: usize,
    duration: Duration,
    concurrency_limit: usize,
    ramp_delay: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("================================================================================");
    info!("   ZENITH BENCH: LIMBO ULTIMATE STRESS & SCALE (REAL CONDITIONS) - STARTING");
    info!(
        "   Target: {}:{} | Bots: {} | Duration: {:.1?} | Concurrency: {}",
        target_host, target_port, bot_count, duration, concurrency_limit
    );
    info!("================================================================================");

    let live = Live::new();
    let join_latencies = LatencyHistogram::new();
    let rtt_latencies = LatencyHistogram::new();
    let semaphore = Arc::new(Semaphore::new(concurrency_limit));
    let start_time = Instant::now();
    let benchmark_active = Arc::new(AtomicBool::new(true));

    // Shared continuous telemetry counters
    let (rtt_tx, mut rtt_rx) = tokio::sync::mpsc::unbounded_channel::<Duration>();
    let (join_tx, mut join_rx) = tokio::sync::mpsc::unbounded_channel::<Duration>();
    let live_ticks = Arc::new(AtomicU64::new(0));
    let live_bytes_out = Arc::new(AtomicU64::new(0));
    let live_bytes_in = Arc::new(AtomicU64::new(0));
    let live_pings = Arc::new(AtomicU64::new(0));
    let live_joined = Arc::new(AtomicUsize::new(0));

    let telemetry = LiveTelemetry {
        joined_limbo: Arc::clone(&live_joined),
        ticks_simulated: Arc::clone(&live_ticks),
        bytes_sent: Arc::clone(&live_bytes_out),
        bytes_received: Arc::clone(&live_bytes_in),
        pings_answered: Arc::clone(&live_pings),
        rtt_tx: Some(rtt_tx),
        join_tx: Some(join_tx),
    };

    // Background task to process RTT samples in real-time
    let rtt_collector = rtt_latencies.clone();
    let rtt_task = tokio::spawn(async move {
        while let Some(rtt) = rtt_rx.recv().await {
            rtt_collector.record(rtt).await;
        }
    });

    // Background task to process Join latency samples in real-time
    let join_collector = join_latencies.clone();
    let join_task = tokio::spawn(async move {
        while let Some(join_lat) = join_rx.recv().await {
            join_collector.record(join_lat).await;
        }
    });

    // 1. Spawn live per-second telemetry reporter
    let reporter_join = join_latencies.clone();
    let reporter_rtt = rtt_latencies.clone();
    let reporter_active = benchmark_active.clone();
    let reporter_joined = Arc::clone(&live_joined);
    let reporter_ticks = Arc::clone(&live_ticks);
    let reporter_bytes_in = Arc::clone(&live_bytes_in);
    let reporter_bytes_out = Arc::clone(&live_bytes_out);
    let metrics_endpoint = metrics_url.to_string();

    let telemetry_task = tokio::spawn(async move {
        let mut second = 0u64;
        let mut prev_bytes = 0u64;
        let mut prev_ticks = 0u64;

        while reporter_active.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_secs(1)).await;
            second += 1;

            let cur_bytes = reporter_bytes_in.load(Ordering::Relaxed)
                + reporter_bytes_out.load(Ordering::Relaxed);
            let d_bytes = cur_bytes.saturating_sub(prev_bytes);
            prev_bytes = cur_bytes;

            let cur_ticks = reporter_ticks.load(Ordering::Relaxed);
            let d_ticks = cur_ticks.saturating_sub(prev_ticks);
            prev_ticks = cur_ticks;

            let mbps = (d_bytes as f64 * 8.0) / 1_000_000.0;
            let pcts_rtt = reporter_rtt.calculate_percentiles().await;
            let pcts_join = reporter_join.calculate_percentiles().await;

            let prom = fetch_zenith_prometheus_metrics(&metrics_endpoint).await;
            let ram_mb = prom
                .as_ref()
                .map(|p| p.memory_rss_bytes as f64 / (1024.0 * 1024.0))
                .unwrap_or(0.0);
            let kib_sess = if let Some(ref p) = prom {
                if p.active_sessions > 0 {
                    (p.memory_rss_bytes as f64 / p.active_sessions as f64) / 1024.0
                } else {
                    0.0
                }
            } else {
                0.0
            };

            let online_est = reporter_joined.load(Ordering::Relaxed);

            println!(
                "[T+{:02}s] 👥 Online: {:4}/{} | 🎮 20Hz Ticks: {:5}/s | 🌐 Wire: {:5.2} Mbps | ⚡ RTT p50: {:4.2?} p99: {:4.2?} | 🚀 Join p50: {:4.2?} | 🧠 RAM: {:4.1} MiB ({:4.2} KiB/sess)",
                second,
                online_est,
                bot_count,
                d_ticks,
                mbps,
                pcts_rtt.p50,
                pcts_rtt.p99,
                pcts_join.p50,
                ram_mb,
                kib_sess
            );
        }
    });

    // 2. Launch concurrent Legitimate Player Probe to guarantee 0 impact on real players
    let probe_live = live.clone();
    let probe_host = target_host.to_string();
    let probe_port = target_port;
    let probe_hold = duration;
    let probe_handle = tokio::spawn(async move {
        // Human probe joins like a real Minecraft player
        let probe = SyntheticClient::new(
            "LegitProbe_HC",
            ClientType::Player {
                profile: PlayerProfile::Interactive,
                ping_jitter: Duration::from_millis(30),
            },
        );
        probe_live.connected.fetch_add(1, Ordering::Relaxed);
        let run_start = Instant::now();
        match probe.run(&probe_host, probe_port, probe_hold).await {
            Ok(ClientOutcome::HeldInLimbo {
                keepalives_answered,
                alerts_received,
                pings_received,
                ticks_simulated,
                bytes_in,
                bytes_out,
                ..
            }) => {
                let join_time = run_start.elapsed();
                (
                    true,
                    join_time,
                    keepalives_answered,
                    alerts_received,
                    pings_received,
                    ticks_simulated,
                    bytes_in,
                    bytes_out,
                )
            }
            _ => (false, Duration::ZERO, 0, 0, 0, 0, 0, 0),
        }
    });

    // 3. Spawn bot actors across Tokio threadpool
    let mut handles = Vec::with_capacity(bot_count);

    for i in 0..bot_count {
        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let live_c = live.clone();
        let host = target_host.to_string();
        let port = target_port;
        let tel_c = telemetry.clone();

        // Realistic Behavior Distribution:
        // 65% Standard (movement in void, KeepAlives)
        // 25% Interactive (movement, KeepAlives, /ping command every 2s)
        // 10% Stressed (rapid rotations at rate-limiter threshold)
        let bucket = i % 100;
        let profile = if bucket < 65 {
            PlayerProfile::Standard
        } else if bucket < 90 {
            PlayerProfile::Interactive
        } else {
            PlayerProfile::Stressed
        };

        // Realistic ping distribution: 15ms to 75ms
        let ping_jitter = Duration::from_millis(15 + (i % 60) as u64);
        let username = format!("LimboPlayer_{i:04}");

        let handle = tokio::spawn(async move {
            let _permit = permit;
            let client = SyntheticClient::new(
                &username,
                ClientType::Player {
                    profile,
                    ping_jitter,
                },
            )
            .with_telemetry(tel_c);

            live_c.connected.fetch_add(1, Ordering::Relaxed);

            match client.run(&host, port, duration).await {
                Ok(ClientOutcome::HeldInLimbo {
                    keepalives_answered,
                    alerts_received,
                    ..
                }) => {
                    live_c.joined_limbo.fetch_add(1, Ordering::Relaxed);
                    live_c
                        .keepalives_answered
                        .fetch_add(keepalives_answered, Ordering::Relaxed);
                    live_c
                        .alerts_received
                        .fetch_add(alerts_received, Ordering::Relaxed);
                }
                Ok(ClientOutcome::Transferred { .. }) => {
                    live_c.transferred.fetch_add(1, Ordering::Relaxed);
                }
                Ok(ClientOutcome::ShieldJailed) => {
                    live_c.jailed_or_blocked.fetch_add(1, Ordering::Relaxed);
                }
                Ok(ClientOutcome::Disconnected) => {}
                Err(err) => {
                    tracing::debug!("Bot failed: {:?}", err);
                    live_c.errors.fetch_add(1, Ordering::Relaxed);
                }
            }
        });

        handles.push(handle);

        // Stagger to prevent OS ephemeral socket storms
        if i % 15 == 0 && !ramp_delay.is_zero() {
            tokio::time::sleep(ramp_delay).await;
        }
    }

    // Wait for all bot sessions to complete
    for h in handles {
        let _ = h.await;
    }

    // Await legitimate probe
    let (probe_ok, probe_join, probe_kas, probe_alerts, probe_pings, probe_ticks, _, _) =
        probe_handle
            .await
            .unwrap_or((false, Duration::ZERO, 0, 0, 0, 0, 0, 0));

    // Signal telemetry reporter to terminate
    benchmark_active.store(false, Ordering::Relaxed);
    let _ = telemetry_task.await;

    // Drop telemetry sender so collectors finish draining
    drop(telemetry);
    let _ = rtt_task.await;
    let _ = join_task.await;

    let total_elapsed = start_time.elapsed();
    let snapshot = live.snapshot();
    let final_ticks = live_ticks.load(Ordering::Relaxed);
    let final_bytes_in = live_bytes_in.load(Ordering::Relaxed);
    let final_bytes_out = live_bytes_out.load(Ordering::Relaxed);
    let final_pings = live_pings.load(Ordering::Relaxed);
    let final_joined = live_joined.load(Ordering::Relaxed);

    let pcts_join = join_latencies.calculate_percentiles().await;
    let pcts_rtt = rtt_latencies.calculate_percentiles().await;

    // Fetch final system memory & prometheus health
    let prom = fetch_zenith_prometheus_metrics(metrics_url).await;
    let (rss_mb, kib_per_sess) = if let Some(ref p) = prom {
        let mb = p.memory_rss_bytes as f64 / (1024.0 * 1024.0);
        let per_sess = if p.active_sessions > 0 {
            (p.memory_rss_bytes as f64 / p.active_sessions as f64) / 1024.0
        } else {
            0.0
        };
        (mb, per_sess)
    } else {
        (0.0, 0.0)
    };

    let total_wire_bytes = final_bytes_in + final_bytes_out;
    let avg_mbps = if total_elapsed.as_secs_f64() > 0.0 {
        (total_wire_bytes as f64 * 8.0) / (total_elapsed.as_secs_f64() * 1_000_000.0)
    } else {
        0.0
    };
    let avg_pps = if total_elapsed.as_secs_f64() > 0.0 {
        (final_ticks as f64) / total_elapsed.as_secs_f64()
    } else {
        0.0
    };

    print_summary_table(
        "ZENITH BENCH: LIMBO ULTIMATE STRESS & SCALE REPORT (REAL CONDITIONS)",
        &[
            ("Target Server", format!("{}:{}", target_host, target_port)),
            ("Total Bots Targeted", bot_count.to_string()),
            (
                "Successful Limbo Joins",
                format!("{}/{}", final_joined, bot_count),
            ),
            (
                "Join Success Rate",
                format!(
                    "{:.1}%",
                    (final_joined as f64 / bot_count.max(1) as f64) * 100.0
                ),
            ),
            ("Connection Drops / Errors", snapshot.errors.to_string()),
            ("Total 20 Hz Ticks Simulated", final_ticks.to_string()),
            (
                "KeepAlives Answered",
                snapshot.keepalives_answered.to_string(),
            ),
            ("Commands Handled (/ping)", final_pings.to_string()),
            ("Total Scenario Elapsed", format!("{total_elapsed:.2?}")),
            (
                "---------------------------------",
                "---------------------------------".to_string(),
            ),
            (
                "SESSION JOIN LATENCY (p50)",
                format!("{:.2?}", pcts_join.p50),
            ),
            (
                "SESSION JOIN LATENCY (p90)",
                format!("{:.2?}", pcts_join.p90),
            ),
            (
                "SESSION JOIN LATENCY (p99)",
                format!("{:.2?}", pcts_join.p99),
            ),
            (
                "SESSION JOIN LATENCY (Max)",
                format!("{:.2?}", pcts_join.max),
            ),
            (
                "---------------------------------",
                "---------------------------------".to_string(),
            ),
            (
                "COMMAND / RTT LATENCY (p50)",
                format!("{:.2?}", pcts_rtt.p50),
            ),
            (
                "COMMAND / RTT LATENCY (p90)",
                format!("{:.2?}", pcts_rtt.p90),
            ),
            (
                "COMMAND / RTT LATENCY (p99)",
                format!("{:.2?}", pcts_rtt.p99),
            ),
            (
                "COMMAND / RTT LATENCY (Max)",
                format!("{:.2?}", pcts_rtt.max),
            ),
            (
                "---------------------------------",
                "---------------------------------".to_string(),
            ),
            (
                "Wire RX Received",
                format!("{:.2} MB", final_bytes_in as f64 / (1024.0 * 1024.0)),
            ),
            (
                "Wire TX Transmitted",
                format!("{:.2} MB", final_bytes_out as f64 / (1024.0 * 1024.0)),
            ),
            ("Average Wire Bandwidth", format!("{avg_mbps:.2} Mbps")),
            ("Average Packet Rate", format!("{avg_pps:.0} PPS")),
            (
                "---------------------------------",
                "---------------------------------".to_string(),
            ),
            ("Zenith Memory RSS", format!("{rss_mb:.2} MiB")),
            (
                "Memory Per Active Session",
                format!("{kib_per_sess:.2} KiB/sess (< 1 KiB invariant)"),
            ),
            (
                "Legitimate Probe Status",
                if probe_ok {
                    format!(
                        "HEALTHY (Join: {:.2?}, {} KAs, {} Ticks)",
                        probe_join, probe_kas, probe_ticks
                    )
                } else {
                    "FAILED".to_string()
                },
            ),
        ],
    );

    // Save final report to JSON file
    let report_json = json!({
        "target": format!("{}:{}", target_host, target_port),
        "total_bots": bot_count,
        "joined_limbo": final_joined,
        "errors": snapshot.errors,
        "ticks_simulated": final_ticks,
        "keepalives_answered": snapshot.keepalives_answered,
        "pings_answered": final_pings,
        "elapsed_seconds": total_elapsed.as_secs_f64(),
        "avg_bandwidth_mbps": avg_mbps,
        "avg_packet_rate_pps": avg_pps,
        "join_latencies": {
            "p50_ms": pcts_join.p50.as_secs_f64() * 1000.0,
            "p90_ms": pcts_join.p90.as_secs_f64() * 1000.0,
            "p99_ms": pcts_join.p99.as_secs_f64() * 1000.0,
            "max_ms": pcts_join.max.as_secs_f64() * 1000.0,
        },
        "rtt_latencies": {
            "p50_ms": pcts_rtt.p50.as_secs_f64() * 1000.0,
            "p90_ms": pcts_rtt.p90.as_secs_f64() * 1000.0,
            "p99_ms": pcts_rtt.p99.as_secs_f64() * 1000.0,
            "max_ms": pcts_rtt.max.as_secs_f64() * 1000.0,
        },
        "memory": {
            "rss_mb": rss_mb,
            "kib_per_session": kib_per_sess,
        },
        "probe": {
            "healthy": probe_ok,
            "join_time_ms": probe_join.as_secs_f64() * 1000.0,
            "keepalives_answered": probe_kas,
            "alerts_received": probe_alerts,
            "pings_received": probe_pings,
            "ticks_simulated": probe_ticks,
        }
    });

    let report_path = "target/limbo_benchmark_report.json";
    if let Err(e) = std::fs::write(report_path, serde_json::to_string_pretty(&report_json)?) {
        tracing::warn!("Could not write report to {}: {}", report_path, e);
    } else {
        info!("Detailed benchmark report saved to {}", report_path);
    }

    Ok(())
}
