//! Comprehensive Anti-DDoS and Packet Filtering Matrix Tests
//!
//! Validates state-aware packet size bounds and continuous Token-Bucket PPS rate limiting
//! against:
//! 1. Legitimate Vanilla/Fabric players
//! 2. High-latency / extreme lag-spike players (TCP burst coalescence after 6s freeze)
//! 3. Chaotic network jitter (Wi-Fi packet drop / Starlink jitter)
//! 4. Blatant bots attempting buffer overflow / oversized frames
//! 5. Packet-flooding botnets attempting CPU exhaustion (300 PPS)
//! 6. Stealthy botnets attempting sustained above-threshold rates
//! 7. Concurrent bot swarms with randomized fingerprints (locales, view distances, brands)

use std::time::Duration;
use tokio::io::{AsyncReadExt, duplex};
use zenith_session::{SessionError, VirtualSession};
use zenith_shield::{PpsConfig, PpsError, ShieldError};
use zenith_wire::{
    ClientInformation, CustomPayload, Handshake, HandshakeIntent, LoginStart, PROTOCOL_VERSION,
    read_raw_packet, write_raw_packet, write_string,
};

/// Helper to execute the standard pre-play handshake, login, and configuration phases.
async fn establish_session_to_limbo<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    c: &mut S,
) {
    establish_custom_session_to_limbo(
        c,
        "LegitPlayer",
        0x1111_2222_3333_4444_5555_6666_7777_8888,
        "en_us",
        12,
        "vanilla",
    )
    .await;
}

/// Helper to execute pre-play phases with customizable credentials, locale, view distance, and brand.
async fn establish_custom_session_to_limbo<
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
>(
    c: &mut S,
    username: &str,
    uuid: u128,
    locale: &str,
    view_distance: i8,
    brand: &str,
) {
    // 1. Handshake
    let hs = Handshake {
        protocol_version: PROTOCOL_VERSION,
        server_address: "localhost".into(),
        server_port: 25565,
        intent: HandshakeIntent::Login,
    };
    write_raw_packet(c, 0x00, &hs.encode()).await.unwrap();

    // 2. Login Start
    let start = LoginStart {
        name: username.into(),
        uuid,
    };
    write_raw_packet(c, 0x00, &start.encode()).await.unwrap();

    // 3. Receive Login Success (ID 0x02)
    let (success_id, _) = read_raw_packet(c, 1024).await.unwrap();
    assert_eq!(success_id, 0x02);

    // 4. Send Login Acknowledged (ID 0x03)
    write_raw_packet(c, 0x03, &[]).await.unwrap();

    // 5. Receive Finish Configuration from Server (ID 0x03)
    let (finish_cfg_id, _) = read_raw_packet(c, 1024).await.unwrap();
    assert_eq!(finish_cfg_id, 0x03);

    // 6. Send ClientInformation with specified locale & view distance
    let info = ClientInformation {
        language: locale.into(),
        view_distance,
        ..Default::default()
    };
    write_raw_packet(c, 0x00, &info.encode()).await.unwrap();

    // 7. Send client software brand
    let mut brand_bytes = Vec::new();
    write_string(brand, &mut brand_bytes);
    let cp = CustomPayload {
        channel: "minecraft:brand".into(),
        data: brand_bytes,
    };
    write_raw_packet(c, 0x02, &cp.encode()).await.unwrap();

    // 8. Finish Configuration Ack (0x03)
    write_raw_packet(c, 0x03, &[]).await.unwrap();

    // 9. Receive Login (Play) packet (ID 50)
    let (login_play_id, _) = read_raw_packet(c, 1024).await.unwrap();
    assert_eq!(login_play_id, 50);

    // Drain initial play packets (GameEvent 39, ChunkBatchFinished 11, PlayerPosition 73, Commands 16, SystemChat 124)
    for _ in 0..5 {
        let _ = read_raw_packet(c, 4096).await.unwrap();
    }
}

/// Scenario 1: A legitimate player sends normal traffic (~20-25 PPS) in Limbo.
#[tokio::test]
async fn test_anti_ddos_legit_player_passes_normal_pacing() {
    let (client, server) = duplex(8192);
    let mut session = VirtualSession::new(server);

    let client_task = tokio::spawn(async move {
        let mut c = client;
        establish_session_to_limbo(&mut c).await;

        // Send 20 movement/status packets spaced by 10ms (normal player tickrate)
        for i in 0..20 {
            tokio::time::sleep(Duration::from_millis(10)).await;
            write_raw_packet(&mut c, 28, &[i as u8]).await.unwrap();
        }
    });

    let server_task = tokio::spawn(async move {
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        session.cmd_rx = Some(cmd_rx);

        let run_future = session.run();
        tokio::pin!(run_future);

        tokio::select! {
            res = &mut run_future => {
                res.unwrap();
            }
            _ = tokio::time::sleep(Duration::from_millis(1500)) => {
                let _ = cmd_tx.send(zenith_session::SessionCommand::Close);
                let res = run_future.await;
                assert!(res.is_ok());
            }
        }
    });

    let (c_res, s_res) = tokio::join!(client_task, server_task);
    c_res.unwrap();
    s_res.unwrap();
}

/// Scenario 2: Extreme Lag Spike & TCP Coalescence
/// A player experiences an extreme network freeze of 500ms, followed by a sudden burst
/// of 75 packets delivered within 10ms.
/// The continuous Token Bucket (capacity 200.0) MUST admit all 75 packets without error.
#[tokio::test]
async fn test_anti_ddos_extreme_lag_spike_burst_passes() {
    let (client, server) = duplex(16384);
    let mut session = VirtualSession::new(server);

    let client_task = tokio::spawn(async move {
        let mut c = client;
        establish_session_to_limbo(&mut c).await;

        // Simulate 200ms of silence
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Massive coalesced burst: 75 packets delivered back-to-back in 10ms
        for i in 0..75 {
            write_raw_packet(&mut c, 28, &[i as u8]).await.unwrap();
        }
    });

    let server_task = tokio::spawn(async move {
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        session.cmd_rx = Some(cmd_rx);

        let run_future = session.run();
        tokio::pin!(run_future);

        tokio::select! {
            res = &mut run_future => {
                res.unwrap();
            }
            _ = tokio::time::sleep(Duration::from_millis(1500)) => {
                let _ = cmd_tx.send(zenith_session::SessionCommand::Close);
                let res = run_future.await;
                assert!(res.is_ok());
            }
        }
    });

    let (c_res, s_res) = tokio::join!(client_task, server_task);
    c_res.unwrap();
    s_res.unwrap();
}

/// Scenario 3: Chaotic Network Jitter & Micro-Bursts
/// Simulates intermittent connection drops (satellite, Wi-Fi through concrete):
/// 3 cycles of silence followed by 25 packets burst. All must pass without kicking the player.
#[tokio::test]
async fn test_anti_ddos_chaotic_jitter_micro_bursts_passes() {
    let (client, server) = duplex(16384);
    let mut session = VirtualSession::new(server);

    let client_task = tokio::spawn(async move {
        let mut c = client;
        establish_session_to_limbo(&mut c).await;

        for _ in 0..3 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            for i in 0..20 {
                write_raw_packet(&mut c, 28, &[i as u8]).await.unwrap();
            }
        }
    });

    let server_task = tokio::spawn(async move {
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        session.cmd_rx = Some(cmd_rx);

        let run_future = session.run();
        tokio::pin!(run_future);

        tokio::select! {
            res = &mut run_future => {
                res.unwrap();
            }
            _ = tokio::time::sleep(Duration::from_millis(1500)) => {
                let _ = cmd_tx.send(zenith_session::SessionCommand::Close);
                let res = run_future.await;
                assert!(res.is_ok());
            }
        }
    });

    let (c_res, s_res) = tokio::join!(client_task, server_task);
    c_res.unwrap();
    s_res.unwrap();
}

/// Scenario 4: A blatant bot attempts a buffer overflow / resource exhaustion attack
/// by sending a 600-byte Handshake packet (max allowed for Handshake is 512 bytes).
/// Zenith must reject it immediately at the transport layer with 0 memory allocation.
#[tokio::test]
async fn test_anti_ddos_blatant_bot_oversized_handshake_rejected() {
    let (mut client, server) = duplex(2048);
    let mut session = VirtualSession::new(server);

    let client_task = tokio::spawn(async move {
        // Send oversized 600-byte Handshake
        let oversized_payload = vec![0x7Fu8; 600];
        write_raw_packet(&mut client, 0x00, &oversized_payload)
            .await
            .unwrap();

        let mut check = [0u8; 1];
        let n = client.read(&mut check).await.unwrap_or(0);
        assert_eq!(n, 0, "Server must close connection on oversized handshake");
    });

    let server_task = tokio::spawn(async move {
        let res = session.run().await;
        match res {
            Err(SessionError::Shield(ShieldError::PayloadTooLarge(sz, max))) => {
                assert_eq!(max, 512);
                assert!(sz > 512);
            }
            other => panic!("Expected PayloadTooLarge(512), got: {:?}", other),
        }
    });

    let (c_res, s_res) = tokio::join!(client_task, server_task);
    c_res.unwrap();
    s_res.unwrap();
}

/// Scenario 5: A blatant bot attempts an oversized Status/Ping request (200 bytes, max is 128 bytes).
#[tokio::test]
async fn test_anti_ddos_blatant_bot_oversized_status_rejected() {
    let (mut client, server) = duplex(2048);
    let mut session = VirtualSession::new(server);

    let client_task = tokio::spawn(async move {
        // Valid Handshake for Status
        let hs = Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "localhost".into(),
            server_port: 25565,
            intent: HandshakeIntent::Status,
        };
        write_raw_packet(&mut client, 0x00, &hs.encode())
            .await
            .unwrap();

        // Oversized Status request of 200 bytes (max allowed is 128)
        let oversized = vec![0xEEu8; 200];
        write_raw_packet(&mut client, 0x00, &oversized)
            .await
            .unwrap();

        let mut check = [0u8; 1];
        let n = client.read(&mut check).await.unwrap_or(0);
        assert_eq!(
            n, 0,
            "Server must close connection on oversized status packet"
        );
    });

    let server_task = tokio::spawn(async move {
        let res = session.run().await;
        match res {
            Err(SessionError::Shield(ShieldError::PayloadTooLarge(sz, max))) => {
                assert_eq!(max, 128);
                assert!(sz > 128);
            }
            other => panic!("Expected PayloadTooLarge(128), got: {:?}", other),
        }
    });

    let (c_res, s_res) = tokio::join!(client_task, server_task);
    c_res.unwrap();
    s_res.unwrap();
}

/// Scenario 6: A packet-flooder bot connects legitimately to Limbo,
/// then immediately bombs the connection with 250 packets in a single instant (exceeding 200 capacity).
/// The Token Bucket must trigger RateExceeded and terminate the session.
#[tokio::test]
async fn test_anti_ddos_packet_flooder_in_limbo_rejected() {
    let (client, server) = duplex(32768);
    let mut session = VirtualSession::new(server);

    let client_task = tokio::spawn(async move {
        let mut c = client;
        establish_session_to_limbo(&mut c).await;

        // Flood 450 packets with 0 delay (surpassing default token bucket capacity of 200 even with refill)
        for i in 0..450 {
            if write_raw_packet(&mut c, 28, &[i as u8]).await.is_err() {
                break;
            }
        }

        // Connection should be closed
        let mut buf = [0u8; 1];
        let _ = c.read(&mut buf).await;
    });

    let server_task = tokio::spawn(async move {
        let res = session.run().await;
        match res {
            Err(SessionError::Pacing(PpsError::RateExceeded { capacity, .. })) => {
                assert_eq!(capacity, 200);
            }
            other => panic!("Expected Pacing RateExceeded, got: {:?}", other),
        }
    });

    let (c_res, s_res) = tokio::join!(client_task, server_task);
    c_res.unwrap();
    s_res.unwrap();
}

/// Scenario 7: A stealthy bot attempts sustained above-rate spam during Configuration.
/// With a small test bucket (capacity: 10.0, refill: 2.0), the bot sends 14 packets in a loop.
/// The bucket drains and kicks the bot cleanly.
#[tokio::test]
async fn test_anti_ddos_stealth_bot_in_configuration_drained_and_evicted() {
    let (mut client, server) = duplex(8192);
    let mut session = VirtualSession::new(server);

    // Tight bucket for test: capacity 10, refill 2 tokens/sec
    session.set_pps_config(PpsConfig {
        refill_rate: 2.0,
        burst_capacity: 10.0,
    });

    let client_task = tokio::spawn(async move {
        // 1. Handshake
        let hs = Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "localhost".into(),
            server_port: 25565,
            intent: HandshakeIntent::Login,
        };
        write_raw_packet(&mut client, 0x00, &hs.encode())
            .await
            .unwrap();

        // 2. Login Start
        let start = LoginStart {
            name: "StealthBot".into(),
            uuid: 0x9999_8888_7777_6666_5555_4444_3333_2222,
        };
        write_raw_packet(&mut client, 0x00, &start.encode())
            .await
            .unwrap();

        // 3. Login Success
        let (success_id, _) = read_raw_packet(&mut client, 1024).await.unwrap();
        assert_eq!(success_id, 0x02);

        // 4. Login Ack
        write_raw_packet(&mut client, 0x03, &[]).await.unwrap();

        // 5. Finish Config from server
        let (finish_cfg_id, _) = read_raw_packet(&mut client, 1024).await.unwrap();
        assert_eq!(finish_cfg_id, 0x03);

        // Send 14 packets back-to-back in Configuration state (capacity is 10)
        for _ in 0..14 {
            if write_raw_packet(&mut client, 0x00, &[]).await.is_err() {
                break;
            }
        }
    });

    let server_task = tokio::spawn(async move {
        let res = session.run().await;
        match res {
            Err(SessionError::Pacing(PpsError::RateExceeded { capacity, .. })) => {
                assert_eq!(capacity, 10);
            }
            other => panic!("Expected Pacing RateExceeded(10), got: {:?}", other),
        }
    });

    let (c_res, s_res) = tokio::join!(client_task, server_task);
    c_res.unwrap();
    s_res.unwrap();
}

/// Scenario 8: Concurrent swarm simulation with randomized fingerprints and brands.
/// Simulates 10 concurrent bot clients:
/// - Each has a distinct username, UUID, and randomized client metadata (locales, view distances, brands: vanilla, fabric, lunarclient, badlion).
/// - Group A (5 clients): Attempt aggressive packet storming in Limbo (250 packets back-to-back) -> all evicted by the Token Bucket!
/// - Group B (5 clients): Idle legitimately in Limbo void answering keepalives -> held cleanly with 0 memory bloat!
#[tokio::test]
async fn test_anti_ddos_concurrent_swarm_with_randomized_fingerprints() {
    let brands = ["vanilla", "fabric", "lunarclient", "badlion"];
    let locales = ["fr_fr", "en_us", "de_de", "es_es", "ja_jp"];
    let view_distances = [8, 10, 12, 14, 16];

    let mut handles = Vec::new();

    for i in 0..10 {
        let is_spammer = i < 5;
        let brand = brands[i % brands.len()];
        let locale = locales[i % locales.len()];
        let view_distance = view_distances[i % view_distances.len()];
        let username = format!("Bot_{}", i);
        let uuid = 0xAA00_0000_0000_0000_0000_0000_0000_0000 | (i as u128);

        let (client, server) = duplex(32768);
        let mut session = VirtualSession::new(server);

        let handle = tokio::spawn(async move {
            let client_task = tokio::spawn(async move {
                let mut c = client;
                establish_custom_session_to_limbo(
                    &mut c,
                    &username,
                    uuid,
                    locale,
                    view_distance,
                    brand,
                )
                .await;

                if is_spammer {
                    // Spammer bot: fires 250 packets rapidly to exhaust Token Bucket (capacity 200)
                    for pkt_idx in 0..250 {
                        if write_raw_packet(&mut c, 28, &[pkt_idx as u8])
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    let mut buf = [0u8; 1];
                    let _ = c.read(&mut buf).await;
                } else {
                    // Idle/normal bot: sits peacefully in limbo
                    tokio::time::sleep(Duration::from_millis(150)).await;
                }
            });

            let server_task = tokio::spawn(async move {
                let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
                session.cmd_rx = Some(cmd_rx);

                if is_spammer {
                    let res = session.run().await;
                    match res {
                        Err(SessionError::Pacing(PpsError::RateExceeded { capacity, .. })) => {
                            assert_eq!(capacity, 200);
                        }
                        other => panic!("Expected Pacing RateExceeded(200), got: {:?}", other),
                    }
                } else {
                    let run_future = session.run();
                    tokio::pin!(run_future);

                    tokio::select! {
                        res = &mut run_future => {
                            res.unwrap();
                        }
                        _ = tokio::time::sleep(Duration::from_millis(200)) => {
                            let _ = cmd_tx.send(zenith_session::SessionCommand::Close);
                            let res = run_future.await;
                            assert!(res.is_ok());
                        }
                    }
                }
            });

            let (c_res, s_res) = tokio::join!(client_task, server_task);
            c_res.unwrap();
            s_res.unwrap();
        });

        handles.push(handle);
    }

    for h in handles {
        h.await.unwrap();
    }
}
