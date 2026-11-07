//! Scenario 2: Priority Queue Drain & Ping-Sorting Evacuation.

use crate::metrics::{Live, print_summary_table};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tracing::info;
use zenith_testkit::{ClientOutcome, ClientType, MockHorizonBackend, SyntheticClient};

pub async fn run_queue_drain(
    target_host: &str,
    target_port: u16,
    backend_addr: SocketAddr,
    bot_count: usize,
    concurrency_limit: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    info!(
        "Starting Queue Drain scenario: {} bots -> Zenith {}:{} -> Mock Backend {}",
        bot_count, target_host, target_port, backend_addr
    );

    // 1. Start Mock Horizon Backend to receive 0x7C transfers
    let mock_backend = MockHorizonBackend::bind(backend_addr, None).await?;
    info!("Mock Horizon Backend active on {}", mock_backend.bind_addr);

    let live = Live::new();
    let semaphore = Arc::new(Semaphore::new(concurrency_limit));
    let start_time = Instant::now();

    let mut handles = Vec::with_capacity(bot_count);

    for i in 0..bot_count {
        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let live_cloned = live.clone();
        let host = target_host.to_string();
        let port = target_port;

        // Distribute VIP names (every 10th bot is VIP) and different ping jitter
        let (username, ping) = if i % 10 == 0 {
            (format!("VIP_Player_{i:04}"), Duration::from_millis(15))
        } else {
            (
                format!("Std_Player_{i:04}"),
                Duration::from_millis(50 + (i % 50) as u64),
            )
        };

        let handle = tokio::spawn(async move {
            let _permit = permit;
            let client = SyntheticClient::new(&username, ClientType::Vanilla { ping_jitter: ping });

            live_cloned
                .connected
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            // Wait for transfer or up to 15 seconds
            match client.run(&host, port, Duration::from_secs(15)).await {
                Ok(ClientOutcome::Transferred {
                    host: dest_h,
                    port: dest_p,
                }) => {
                    live_cloned
                        .transferred
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                    // Client seamlessly joins the target backend!
                    let backend_client = SyntheticClient::new(&username, ClientType::Horizon);
                    let _ = backend_client
                        .run(&dest_h, dest_p, Duration::from_secs(5))
                        .await;
                }
                Ok(ClientOutcome::HeldInLimbo { .. }) => {
                    live_cloned
                        .joined_limbo
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
        if i % 50 == 0 {
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    }

    // Wait for all bots
    for h in handles {
        let _ = h.await;
    }

    let elapsed = start_time.elapsed();
    let snapshot = live.snapshot();
    let backend_completed = mock_backend
        .stats
        .transfers_completed
        .load(std::sync::atomic::Ordering::Relaxed);
    let slp_queries = mock_backend
        .stats
        .slp_requests
        .load(std::sync::atomic::Ordering::Relaxed);

    let drain_rate = if elapsed.as_secs_f64() > 0.0 {
        snapshot.transferred as f64 / elapsed.as_secs_f64()
    } else {
        0.0
    };

    print_summary_table(
        "ZENITH BENCH: SCENARIO 2 - PRIORITY QUEUE DRAIN REPORT",
        &[
            ("Total Bots Queued", bot_count.to_string()),
            (
                "Transfers Received by Bots",
                snapshot.transferred.to_string(),
            ),
            (
                "Transfers Ingested by Backend",
                backend_completed.to_string(),
            ),
            ("Totem SLP Probes Answered", slp_queries.to_string()),
            (
                "Average Drain Throughput",
                format!("{drain_rate:.2} players/sec"),
            ),
            ("Connection Errors", snapshot.errors.to_string()),
            ("Total Evacuation Duration", format!("{elapsed:.2?}")),
        ],
    );

    mock_backend.shutdown();
    Ok(())
}
