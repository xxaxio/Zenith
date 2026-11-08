//! Scenario: Overhead A/B - Microsecond-exact Differential Latency Measurement.
//!
//! Measures the roundtrip packet latency DIRECT to a stand-in backend, then
//! measures the roundtrip latency PROXIED through Zenith.
//! Computes the exact added overhead in microsecond percentiles (P50, P90, P99, Max).

use crate::metrics::{LatencyHistogram, LatencyPercentiles};
use bytes::BytesMut;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tracing::info;
use zenith_wire::{
    Handshake, HandshakeIntent, LoginStart, PROTOCOL_VERSION, read_raw_packet, write_raw_packet,
    write_string, write_varint,
};

const BENCH_CHANNEL: &str = "zenith:bench";

/// Runs the A/B comparative benchmark between Direct backend and Zenith.
pub async fn run_overhead_ab(
    zenith_addr: &str,
    players: usize,
    sample_rate: u32,
    duration_secs: u64,
    concurrency: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("================================================================================");
    info!("   ZENITH BENCH: DIFFERENTIAL A/B OVERHEAD BENCHMARK (MICROSECOND EXACT)");
    info!(
        "   Players: {} | Rate: {} pkt/s | Window: {}s | Concurrency: {}",
        players, sample_rate, duration_secs, concurrency
    );
    info!("================================================================================");

    // 1. Start synthetic stand-in echo backend on ephemeral local port
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let backend_addr = listener.local_addr()?;
    info!("Stand-in echo backend listening on {}", backend_addr);

    let backend_handle = tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let _ = handle_standin_backend(&mut socket).await;
            });
        }
    });

    // 2. Phase A: Direct phase (Client -> Backend directly)
    info!(
        "[PHASE A] Measuring DIRECT baseline latency against stand-in backend ({} players)...",
        players
    );
    let direct_hist = run_echo_phase(
        &backend_addr.to_string(),
        players,
        sample_rate,
        Duration::from_secs(duration_secs),
        concurrency,
        false,
    )
    .await?;

    // 3. Phase B: Proxied phase through Zenith
    info!(
        "[PHASE B] Measuring PROXIED latency through Zenith ({}) with {} players...",
        zenith_addr, players
    );
    let proxied_hist = run_echo_phase(
        zenith_addr,
        players,
        sample_rate,
        Duration::from_secs(duration_secs),
        concurrency,
        true,
    )
    .await?;

    backend_handle.abort();

    // 4. Calculate percentiles and print comparative summary report
    let direct_perc = direct_hist.calculate_percentiles().await;
    let proxied_perc = proxied_hist.calculate_percentiles().await;
    print_ab_report(&direct_perc, &proxied_perc, players);

    Ok(())
}

async fn handle_standin_backend(socket: &mut TcpStream) -> Result<(), String> {
    // 1. Handshake
    let (_hs_id, _hs_payload) = read_raw_packet(socket, 1024)
        .await
        .map_err(|e| e.to_string())?;
    // 2. Login Start
    let (_login_id, login_payload) = read_raw_packet(socket, 2048)
        .await
        .map_err(|e| e.to_string())?;
    let start = LoginStart::decode(&login_payload).map_err(|e| e.to_string())?;

    // 3. Login Success
    let mut success = BytesMut::new();
    success.extend_from_slice(&start.uuid.to_be_bytes());
    write_string(&start.name, &mut success);
    write_varint(0, &mut success);
    success.extend_from_slice(&start.uuid.to_be_bytes());
    write_raw_packet(socket, 0x02, &success)
        .await
        .map_err(|e| e.to_string())?;

    // 4. Login Ack
    let (_ack_id, _) = read_raw_packet(socket, 1024)
        .await
        .map_err(|e| e.to_string())?;

    // 5. Config Finish
    write_raw_packet(socket, 0x03, &[])
        .await
        .map_err(|e| e.to_string())?;
    let (_cfg_ack, _) = read_raw_packet(socket, 1024)
        .await
        .map_err(|e| e.to_string())?;

    // 6. Play state: Echo loop for custom payload or KeepAlive packets
    loop {
        let (id, payload) = match read_raw_packet(socket, 32_768).await {
            Ok(res) => res,
            Err(_) => break,
        };
        // Echo measuring packet straight back
        if write_raw_packet(socket, id, &payload).await.is_err() {
            break;
        }
    }
    Ok(())
}

async fn run_echo_phase(
    target: &str,
    players: usize,
    sample_rate: u32,
    window: Duration,
    concurrency: usize,
    is_zenith: bool,
) -> Result<Arc<LatencyHistogram>, Box<dyn std::error::Error>> {
    let hist = LatencyHistogram::new();
    let sem = Arc::new(Semaphore::new(concurrency));
    let mut handles = Vec::with_capacity(players);

    let (rtt_tx, mut rtt_rx) = tokio::sync::mpsc::unbounded_channel::<Duration>();

    // Collector task
    let hist_collector = {
        let hist_clone = Arc::clone(&hist);
        tokio::spawn(async move {
            while let Some(rtt) = rtt_rx.recv().await {
                hist_clone.record(rtt).await;
            }
        })
    };

    let interval_micros = if sample_rate > 0 {
        1_000_000 / sample_rate as u64
    } else {
        50_000
    };

    for idx in 0..players {
        let permit = sem.clone().acquire_owned().await?;
        let target_str = target.to_string();
        let rtt_tx_clone = rtt_tx.clone();

        let handle = tokio::spawn(async move {
            let _permit = permit;
            let mut stream = match TcpStream::connect(&target_str).await {
                Ok(s) => s,
                Err(_) => return,
            };

            // Complete Handshake + Login
            let hs = Handshake {
                protocol_version: PROTOCOL_VERSION,
                server_address: "127.0.0.1".into(),
                server_port: 25565,
                intent: HandshakeIntent::Login,
            };
            if write_raw_packet(&mut stream, 0x00, &hs.encode())
                .await
                .is_err()
            {
                return;
            }

            let start = LoginStart {
                name: format!("Bench_{idx}"),
                uuid: 0xDEAD_BEEF_0000_0000 | (idx as u128),
            };
            if write_raw_packet(&mut stream, 0x00, &start.encode())
                .await
                .is_err()
            {
                return;
            }

            // Read LoginSuccess + send LoginAck
            if read_raw_packet(&mut stream, 2048).await.is_err() {
                return;
            }
            if write_raw_packet(&mut stream, 0x03, &[]).await.is_err() {
                return;
            }

            // Config: read FinishConfig + send ConfigAck
            if read_raw_packet(&mut stream, 2048).await.is_err() {
                return;
            }
            if write_raw_packet(&mut stream, 0x03, &[]).await.is_err() {
                return;
            }

            // If connecting through Zenith, consume Limbo entry packet burst
            if is_zenith {
                // Drain initial Limbo burst (LoginPlay, GameEvent, etc.)
                let _ = read_raw_packet(&mut stream, 8192).await;
            }

            // Measurement phase
            let deadline = Instant::now() + window;
            let mut seq: u64 = 0;

            while Instant::now() < deadline {
                seq += 1;
                let t0 = Instant::now();

                // Send measuring payload on "zenith:bench" channel
                let mut payload = BytesMut::new();
                write_string(BENCH_CHANNEL, &mut payload);
                payload.extend_from_slice(&seq.to_be_bytes());
                payload.extend_from_slice(&[0xAA; 48]); // filler up to 64 bytes

                // Send packet ID 0x16 (CustomPayload / PluginMessage) or KeepAlive
                if write_raw_packet(&mut stream, 0x16, &payload).await.is_err() {
                    break;
                }

                // Wait for echo / pong
                match read_raw_packet(&mut stream, 2048).await {
                    Ok(_) => {
                        let rtt = t0.elapsed();
                        let _ = rtt_tx_clone.send(rtt);
                    }
                    Err(_) => break,
                }

                tokio::time::sleep(Duration::from_micros(interval_micros)).await;
            }
        });

        handles.push(handle);
    }

    drop(rtt_tx);
    for h in handles {
        let _ = h.await;
    }
    let _ = hist_collector.await;

    Ok(hist)
}

fn print_ab_report(direct: &LatencyPercentiles, proxied: &LatencyPercentiles, players: usize) {
    let d_p50 = direct.p50.as_micros();
    let d_p90 = direct.p90.as_micros();
    let d_p99 = direct.p99.as_micros();
    let d_max = direct.max.as_micros();

    let p_p50 = proxied.p50.as_micros();
    let p_p90 = proxied.p90.as_micros();
    let p_p99 = proxied.p99.as_micros();
    let p_max = proxied.max.as_micros();

    let diff_p50 = p_p50 as i64 - d_p50 as i64;
    let diff_p90 = p_p90 as i64 - d_p90 as i64;
    let diff_p99 = p_p99 as i64 - d_p99 as i64;

    info!("");
    info!("================================================================================");
    info!(
        "   ZENITH DIFFERENTIAL A/B BENCHMARK REPORT ({} SIMULTANEOUS PLAYERS)",
        players
    );
    info!("================================================================================");
    info!("| Configuration | Samples  | P50 (µs)   | P90 (µs)   | P99 (µs)   | Max (µs)   |");
    info!("|---------------|----------|------------|------------|------------|------------|");
    info!(
        "| Direct (Echo) | {:<8} | {:<10} | {:<10} | {:<10} | {:<10} |",
        direct.count,
        format!("{d_p50} µs"),
        format!("{d_p90} µs"),
        format!("{d_p99} µs"),
        format!("{d_max} µs")
    );
    info!(
        "| Zenith Proxied| {:<8} | {:<10} | {:<10} | {:<10} | {:<10} |",
        proxied.count,
        format!("{p_p50} µs"),
        format!("{p_p90} µs"),
        format!("{p_p99} µs"),
        format!("{p_max} µs")
    );
    info!(
        "| Overhead Δ    |    --    | {:<10} | {:<10} | {:<10} |    --      |",
        format!("{:+}", diff_p50),
        format!("{:+}", diff_p90),
        format!("{:+}", diff_p99)
    );
    info!("================================================================================");
    info!(
        "VERDICT: Zenith added overhead at P50 is {:+} µs, P99 is {:+} µs.",
        diff_p50, diff_p99
    );
    info!("================================================================================");
}
