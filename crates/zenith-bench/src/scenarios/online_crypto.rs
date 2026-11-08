//! Scenario: Online Mode Mojang Cryptography & Encryption Overhead Benchmark.
//!
//! Evaluates the computational throughput, latency percentiles, and memory consumption
//! of full online mode: RSA 1024-bit key exchange, challenge nonce verification,
//! AES-128 CFB8 stream cipher encryption/decryption, and Mojang session server validation.

use crate::metrics::LatencyHistogram;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tracing::info;
use zenith_session::{OnlineModeConfig, VirtualSession};
use zenith_shield::ShieldLimits;
use zenith_wire::crypto::{
    EncryptedStream, EncryptionRequest, EncryptionResponse, RsaKeypair, generate_shared_secret,
    rsa_encrypt,
};
use zenith_wire::{
    ClientInformation, Handshake, HandshakeIntent, KeepAlive, LoginStart, PROTOCOL_VERSION,
    read_raw_packet, write_raw_packet,
};

/// High-throughput mock Mojang session verification service.
struct MockAuthService {
    pub addr: String,
    pub verifications: Arc<AtomicUsize>,
}

impl MockAuthService {
    pub async fn spawn(simulated_latency: Duration) -> Result<Self, Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let local_addr = listener.local_addr()?;
        let addr = format!("http://127.0.0.1:{}", local_addr.port());
        let verifications = Arc::new(AtomicUsize::new(0));
        let verif_clone = Arc::clone(&verifications);

        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let verif_task = Arc::clone(&verif_clone);
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let n = match socket.read(&mut buf).await {
                        Ok(n) if n > 0 => n,
                        _ => return,
                    };
                    let req = String::from_utf8_lossy(&buf[..n]);
                    verif_task.fetch_add(1, Ordering::Relaxed);

                    if simulated_latency > Duration::ZERO {
                        tokio::time::sleep(simulated_latency).await;
                    }

                    // Extract username parameter
                    let username = req
                        .split("username=")
                        .nth(1)
                        .and_then(|s| s.split('&').next())
                        .unwrap_or("OnlinePlayer");

                    let body = format!(
                        r#"{{"id":"5f3c1e6a2b9d4c7e8a1f0b2c3d4e5f60","name":"{username}","properties":[{{"name":"textures","value":"eyJ0ZXh0dXJlcyI6e319"}}]}}"#
                    );

                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.flush().await;
                });
            }
        });

        Ok(Self {
            addr,
            verifications,
        })
    }
}

/// Runs the complete Online Mode Cryptography benchmark suite.
pub async fn run_online_crypto_benchmark(
    bot_count: usize,
    duration_secs: u64,
    concurrency_limit: usize,
    mojang_delay_ms: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("================================================================================");
    info!("   ZENITH BENCH: FULL ONLINE MOJANG CRYPTO & AES-128 CFB8 BENCHMARK");
    info!(
        "   Bots: {} | Concurrency: {} | Window: {}s | Mojang API Latency: {}ms",
        bot_count, concurrency_limit, duration_secs, mojang_delay_ms
    );
    info!("================================================================================");

    // -------------------------------------------------------------------------
    // Phase 1: Micro-Benchmark - Raw RSA 1024-bit Decryption Throughput
    // -------------------------------------------------------------------------
    info!("[PHASE 1] Benchmarking raw RSA-1024 keypair generation & PKCS#1 v1.5 decryption...");
    let gen_start = Instant::now();
    let server_keypair = Arc::new(RsaKeypair::generate()?);
    let gen_dur = gen_start.elapsed();
    info!("   RSA-1024 keypair generated in {:.2?}", gen_dur);

    let rsa_hist = LatencyHistogram::new();
    let rsa_iterations = 500;
    let secret_sample = [0x42u8; 16];
    let encrypted_secret = rsa_encrypt(server_keypair.public_der(), &secret_sample)?;

    let rsa_start = Instant::now();
    for _ in 0..rsa_iterations {
        let t0 = Instant::now();
        let decrypted = server_keypair.decrypt(&encrypted_secret)?;
        rsa_hist.record(t0.elapsed()).await;
        assert_eq!(&decrypted[..], &secret_sample[..]);
    }
    let rsa_total = rsa_start.elapsed();
    let rsa_p = rsa_hist.calculate_percentiles().await;
    let rsa_ops_per_sec = (rsa_iterations as f64) / rsa_total.as_secs_f64();

    info!(
        "   RSA Decrypt (500 ops): P50: {:?} | P90: {:?} | P99: {:?} | Throughput: {:.1} ops/sec",
        rsa_p.p50, rsa_p.p90, rsa_p.p99, rsa_ops_per_sec
    );

    // -------------------------------------------------------------------------
    // Phase 2: Micro-Benchmark - AES-128 CFB8 Stream Cipher Throughput
    // -------------------------------------------------------------------------
    info!("[PHASE 2] Benchmarking AES-128 CFB8 stream cipher encryption throughput...");
    let key = [0x5Au8; 16];
    let test_frame_sizes = [64, 128, 512, 1024, 2048];
    for &size in &test_frame_sizes {
        let (tx_pipe, rx_pipe) = tokio::io::duplex(64 * 1024);
        let mut enc_stream = EncryptedStream::new(tx_pipe);
        let mut dec_stream = EncryptedStream::new(rx_pipe);
        enc_stream.enable_encryption(&key);
        dec_stream.enable_encryption(&key);

        let frame = vec![0xABu8; size];
        let iterations = 20_000;
        let mut read_buf = vec![0u8; size];

        let pipe_start = Instant::now();
        let sender = tokio::spawn(async move {
            for _ in 0..iterations {
                enc_stream.write_all(&frame).await.unwrap();
            }
            enc_stream.flush().await.unwrap();
        });

        let receiver = tokio::spawn(async move {
            for _ in 0..iterations {
                dec_stream.read_exact(&mut read_buf).await.unwrap();
            }
        });

        sender.await?;
        receiver.await?;
        let elapsed = pipe_start.elapsed();

        let total_bytes = (size * iterations) as f64;
        let mb_per_sec = (total_bytes / 1024.0 / 1024.0) / elapsed.as_secs_f64();
        let pkts_per_sec = (iterations as f64) / elapsed.as_secs_f64();
        let us_per_pkt = (elapsed.as_micros() as f64) / (iterations as f64);

        info!(
            "   Frame {:4} B: {:8.1} MB/s | {:6.0} pkts/s | {:5.2} µs/pkt overhead",
            size, mb_per_sec, pkts_per_sec, us_per_pkt
        );
    }

    // -------------------------------------------------------------------------
    // Phase 3: End-to-End Online Handshake & Limbo Holding Load Test
    // -------------------------------------------------------------------------
    info!(
        "[PHASE 3] Launching End-to-End Online Handshake & Limbo holding with {} bots...",
        bot_count
    );

    // Spawn mock Mojang auth service with realistic latency
    let mock_auth = MockAuthService::spawn(Duration::from_millis(mojang_delay_ms)).await?;
    info!("   Mock Mojang SessionServer active at {}", mock_auth.addr);

    // Spawn standalone Zenith Online Mode listener
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let zenith_port = listener.local_addr()?.port();
    let zenith_addr = format!("127.0.0.1:{}", zenith_port);
    info!("   Zenith Online Mode listener active on {}", zenith_addr);

    let srv_keypair = Arc::clone(&server_keypair);
    let auth_url = mock_auth.addr.clone();
    let server_shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_signal = Arc::clone(&server_shutdown);

    let server_handle = tokio::spawn(async move {
        while !shutdown_signal.load(Ordering::Relaxed) {
            match listener.accept().await {
                Ok((socket, _)) => {
                    let kp = Arc::clone(&srv_keypair);
                    let url = auth_url.clone();
                    tokio::spawn(async move {
                        let stream = EncryptedStream::new(socket);
                        let mut session =
                            VirtualSession::with_limits(stream, ShieldLimits::default());
                        session.set_online_mode(OnlineModeConfig::Online {
                            keypair: kp,
                            auth_endpoint: Some(url),
                            prevent_proxy_connections: false,
                        });
                        let _ = session.run().await;
                    });
                }
                Err(_) => break,
            }
        }
    });

    let join_latencies = LatencyHistogram::new();
    let keepalive_latencies = LatencyHistogram::new();
    let semaphore = Arc::new(Semaphore::new(concurrency_limit));
    let active_sessions = Arc::new(AtomicUsize::new(0));
    let successful_logins = Arc::new(AtomicUsize::new(0));
    let total_bytes_streamed = Arc::new(AtomicU64::new(0));

    let bench_start = Instant::now();
    let bench_duration = Duration::from_secs(duration_secs);

    let mut bot_handles = Vec::with_capacity(bot_count);
    for idx in 0..bot_count {
        let sem = Arc::clone(&semaphore);
        let target = zenith_addr.clone();
        let join_lat = join_latencies.clone();
        let ka_lat = keepalive_latencies.clone();
        let active = Arc::clone(&active_sessions);
        let succ = Arc::clone(&successful_logins);
        let bytes_counter = Arc::clone(&total_bytes_streamed);

        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            let bot_name = format!("Bot_{:04}", idx);

            let t_conn_start = Instant::now();
            let socket = match TcpStream::connect(&target).await {
                Ok(s) => s,
                Err(_) => return,
            };

            let mut stream = EncryptedStream::new(socket);

            // 1. Handshake
            let hs = Handshake {
                protocol_version: PROTOCOL_VERSION,
                server_address: "play.horizon.dev".into(),
                server_port: 25565,
                intent: HandshakeIntent::Login,
            };
            let hs_bytes = hs.encode();
            if write_raw_packet(&mut stream, 0x00, &hs_bytes)
                .await
                .is_err()
            {
                return;
            }

            // 2. LoginStart
            let start = LoginStart {
                name: bot_name,
                uuid: 0,
            };
            let start_bytes = start.encode();
            if write_raw_packet(&mut stream, 0x00, &start_bytes)
                .await
                .is_err()
            {
                return;
            }

            // 3. Receive EncryptionRequest (0x01)
            let (req_id, req_payload) = match read_raw_packet(&mut stream, 2048).await {
                Ok(res) => res,
                Err(_) => return,
            };
            if req_id != 0x01 {
                return;
            }

            let enc_req = match EncryptionRequest::decode(&req_payload) {
                Ok(r) => r,
                Err(_) => return,
            };

            // 4. Generate shared secret & encrypt
            let secret = match generate_shared_secret() {
                Ok(s) => s,
                Err(_) => return,
            };

            let enc_secret = match rsa_encrypt(&enc_req.public_key, &secret) {
                Ok(s) => s,
                Err(_) => return,
            };
            let enc_challenge = match rsa_encrypt(&enc_req.public_key, &enc_req.challenge) {
                Ok(c) => c,
                Err(_) => return,
            };

            // 5. Send EncryptionResponse (0x01)
            let enc_resp = EncryptionResponse {
                shared_secret: enc_secret,
                challenge: enc_challenge,
            };
            if write_raw_packet(&mut stream, 0x01, &enc_resp.encode())
                .await
                .is_err()
            {
                return;
            }

            // 6. Enable AES-128 CFB8 stream cipher
            stream.enable_encryption(&secret);

            // 7. Receive encrypted LoginSuccess (0x02)
            let (succ_id, _) = match read_raw_packet(&mut stream, 2048).await {
                Ok(res) => res,
                Err(_) => return,
            };
            if succ_id != 0x02 {
                return;
            }

            // 8. Send encrypted LoginAcknowledged (0x03)
            if write_raw_packet(&mut stream, 0x03, &[]).await.is_err() {
                return;
            }

            // 9. Read FinishConfiguration (0x03)
            let (cfg_id, _) = match read_raw_packet(&mut stream, 2048).await {
                Ok(res) => res,
                Err(_) => return,
            };
            if cfg_id != 0x03 {
                return;
            }

            // 10. Send ClientInformation & FinishConfiguration
            let info = ClientInformation::default();
            if write_raw_packet(&mut stream, 0x00, &info.encode())
                .await
                .is_err()
            {
                return;
            }
            if write_raw_packet(&mut stream, 0x03, &[]).await.is_err() {
                return;
            }

            let join_duration = t_conn_start.elapsed();
            join_lat.record(join_duration).await;
            succ.fetch_add(1, Ordering::Relaxed);
            active.fetch_add(1, Ordering::Relaxed);

            // 11. Limbo encrypted packet holding loop
            let mut read_buf = [0u8; 4096];
            let end_time = t_conn_start + bench_duration;

            while Instant::now() < end_time {
                tokio::select! {
                    res = stream.read(&mut read_buf) => {
                        match res {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                bytes_counter.fetch_add(n as u64, Ordering::Relaxed);
                            }
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(500)) => {
                        // Send encrypted KeepAlive ping
                        let t_ka = Instant::now();
                        let ping = KeepAlive { id: 0x41424344 };
                        if write_raw_packet(&mut stream, 0x18, &ping.encode()).await.is_err() {
                            break;
                        }
                        ka_lat.record(t_ka.elapsed()).await;
                    }
                }
            }

            active.fetch_sub(1, Ordering::Relaxed);
        });

        bot_handles.push(handle);
    }

    // Live reporter loop
    let reporter_active = Arc::clone(&active_sessions);
    let reporter_succ = Arc::clone(&successful_logins);
    let reporter_bytes = Arc::clone(&total_bytes_streamed);

    let reporter = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        let mut sec = 0;
        loop {
            interval.tick().await;
            sec += 1;
            let online = reporter_active.load(Ordering::Relaxed);
            let total_succ = reporter_succ.load(Ordering::Relaxed);
            let mbps =
                (reporter_bytes.load(Ordering::Relaxed) as f64 * 8.0) / 1_000_000.0 / (sec as f64);
            info!(
                "[T+{:02}s] 🔐 Online Crypto Bots: {:4} | Completed: {:4}/{} | Wire: {:5.2} Mbps",
                sec, online, total_succ, bot_count, mbps
            );
            if sec >= duration_secs + 2 {
                break;
            }
        }
    });

    for handle in bot_handles {
        let _ = handle.await;
    }
    let _ = reporter.await;

    server_shutdown.store(true, Ordering::Relaxed);
    server_handle.abort();

    // -------------------------------------------------------------------------
    // Phase 4: Final Metrics Summary
    // -------------------------------------------------------------------------
    let total_elapsed = bench_start.elapsed();
    let completed = successful_logins.load(Ordering::Relaxed);
    let join_p = join_latencies.calculate_percentiles().await;
    let ka_p = keepalive_latencies.calculate_percentiles().await;
    let verifications = mock_auth.verifications.load(Ordering::Relaxed);

    info!("================================================================================");
    info!("   ZENITH ONLINE MOJANG CRYPTO BENCHMARK: FINAL RESULTS");
    info!("================================================================================");
    info!("   Total Handshakes Attempted: {}", bot_count);
    info!(
        "   Successful Authentications: {} ({:.1}%)",
        completed,
        (completed as f64 / bot_count as f64) * 100.0
    );
    info!("   Mojang HTTP Verifications: {}", verifications);
    info!("   Total Benchmark Duration:   {:.2?}", total_elapsed);
    info!("--------------------------------------------------------------------------------");
    info!("   CRYPTO LATENCY PERCENTILES:");
    info!(
        "   • RSA-1024 Decrypt:        P50: {:?} | P90: {:?} | P99: {:?}",
        rsa_p.p50, rsa_p.p90, rsa_p.p99
    );
    info!(
        "   • Online Handshake (E2E):  P50: {:?} | P90: {:?} | P99: {:?} | Max: {:?}",
        join_p.p50, join_p.p90, join_p.p99, join_p.max
    );
    info!(
        "   • KeepAlive (AES-128):     P50: {:?} | P90: {:?} | P99: {:?} | Max: {:?}",
        ka_p.p50, ka_p.p90, ka_p.p99, ka_p.max
    );
    info!("================================================================================");

    Ok(())
}
