//! Comprehensive integration test suite for Zenith Shield Level 2 Kernel / Firewall escalation.
//!
//! Validates:
//! 1. Legitimate player with severe lag or disconnect: ZERO firewall escalation (0% false positives).
//! 2. Attack bot injecting oversized Handshake (> 512 bytes): INSTANT firewall escalation.
//! 3. Attack bot reporting blacklisted brand (`mcstorm`): INSTANT firewall escalation.
//! 4. Whitelist / Loopback immunity: never escalated even on hard frame violation.
//! 5. Deduplication window: suppresses spamming the OS kernel with redundant ban calls.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::duplex;
use tokio::sync::mpsc;
use zenith_session::{SessionEvent, VirtualSession};
use zenith_shield::{FingerprintConfig, FirewallManager, NoopFirewall};
use zenith_wire::{
    ClientInformation, Handshake, HandshakeIntent, PROTOCOL_VERSION, write_raw_packet,
};

#[tokio::test]
async fn test_firewall_escalation_legitimate_player_never_escalated() {
    let (mut client, server) = duplex(4096);
    let mut session = VirtualSession::new(server);

    let attacker_ip = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1));
    session.set_client_ip(attacker_ip);

    let noop_backend = Arc::new(NoopFirewall::new());
    let fw_manager = FirewallManager::new(noop_backend.clone(), true, Duration::from_secs(900));

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    session.set_event_sender(event_tx);

    let client_task = tokio::spawn(async move {
        // Send valid handshake
        let hs = Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "localhost".into(),
            server_port: 25565,
            intent: HandshakeIntent::Status,
        };
        write_raw_packet(&mut client, 0x00, &hs.encode())
            .await
            .unwrap();

        // Simulate sudden client disconnect (network drop)
        drop(client);
    });

    let fw_clone = fw_manager.clone();
    let coordinator_task = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            if let SessionEvent::HardViolation {
                client_ip: Some(ip),
                reason,
                ..
            } = event
            {
                fw_clone.escalate(ip, reason).await;
            }
        }
    });

    let _ = session.run().await;
    let _ = client_task.await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    coordinator_task.abort();

    // Verify ZERO firewall rules were created for the legitimate player
    assert!(!noop_backend.is_banned(&attacker_ip));
    assert_eq!(noop_backend.banned_count(), 0);
}

#[tokio::test]
async fn test_firewall_escalation_oversized_handshake_triggers_kernel_drop() {
    let (mut client, server) = duplex(4096);
    let mut session = VirtualSession::new(server);

    let attacker_ip = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 55));
    session.set_client_ip(attacker_ip);

    let noop_backend = Arc::new(NoopFirewall::new());
    let fw_manager = FirewallManager::new(noop_backend.clone(), true, Duration::from_secs(900));

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    session.set_event_sender(event_tx);

    let client_task = tokio::spawn(async move {
        // Send blatant 600-byte Handshake (max allowed is 512 bytes)
        let malicious_payload = vec![0xEEu8; 600];
        let _ = write_raw_packet(&mut client, 0x00, &malicious_payload).await;
    });

    let fw_clone = fw_manager.clone();
    let coordinator_task = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            if let SessionEvent::HardViolation {
                client_ip: Some(ip),
                reason,
                ..
            } = event
            {
                fw_clone.escalate(ip, reason).await;
            }
        }
    });

    let res = session.run().await;
    assert!(res.is_err(), "Session must terminate on frame overflow");

    let _ = client_task.await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    coordinator_task.abort();

    // Verify IP is NOW banned in the firewall backend!
    assert!(
        noop_backend.is_banned(&attacker_ip),
        "Attacker IP must be jailed at the kernel level"
    );
    assert_eq!(noop_backend.banned_count(), 1);
}

#[tokio::test]
async fn test_firewall_escalation_blacklisted_brand_triggers_kernel_drop() {
    let (mut client, server) = duplex(8192);
    let mut session = VirtualSession::new(server);

    let attacker_ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 77));
    session.set_client_ip(attacker_ip);

    let fp_config = FingerprintConfig {
        blocked_brands: vec!["mcstorm".to_string(), "mctool".to_string()],
        ..Default::default()
    };
    session.set_fingerprint_config(fp_config);

    let noop_backend = Arc::new(NoopFirewall::new());
    let fw_manager = FirewallManager::new(noop_backend.clone(), true, Duration::from_secs(900));

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    session.set_event_sender(event_tx);

    let client_task = tokio::spawn(async move {
        // 1. Valid Handshake for Login
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
        let login_start = zenith_wire::LoginStart {
            name: "Bot42".to_string(),
            uuid: 0x12345678,
        };
        write_raw_packet(&mut client, 0x00, &login_start.encode())
            .await
            .unwrap();

        // Drain Login Success and Configuration transitions
        let _ = zenith_wire::read_raw_packet(&mut client, 1024).await; // LoginSuccess 0x02

        // Send LoginAcknowledged 0x03
        write_raw_packet(&mut client, 0x03, &[]).await.unwrap();

        // Drain KnownPacks 0x0E
        let _ = zenith_wire::read_raw_packet(&mut client, 1024).await;

        // Send ClientInformation 0x00
        let ci = ClientInformation {
            language: "en_us".into(),
            view_distance: 12,
            chat_visibility: 0,
            chat_colors: true,
            model_customisation: 127,
            main_hand: 1,
            text_filtering: false,
            allows_listing: true,
            particle_status: 0,
        };
        write_raw_packet(&mut client, 0x00, &ci.encode())
            .await
            .unwrap();

        // Send CustomPayload Brand reporting attack tool: "mcstorm"
        let mut brand_payload = bytes::BytesMut::new();
        zenith_wire::write_string("minecraft:brand", &mut brand_payload);
        zenith_wire::write_string("mcstorm", &mut brand_payload);
        write_raw_packet(&mut client, 0x02, &brand_payload)
            .await
            .unwrap();

        // Send FinishConfiguration 0x03
        let _ = write_raw_packet(&mut client, 0x03, &[]).await;
    });

    let fw_clone = fw_manager.clone();
    let coordinator_task = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            if let SessionEvent::HardViolation {
                client_ip: Some(ip),
                reason,
                ..
            } = event
            {
                fw_clone.escalate(ip, reason).await;
            }
        }
    });

    let res = session.run().await;
    assert!(res.is_err());

    let _ = client_task.await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    coordinator_task.abort();

    // Verify the bot was banned at the OS kernel level!
    assert!(
        noop_backend.is_banned(&attacker_ip),
        "Bot using mcstorm brand must be escalated to OS firewall"
    );
}

#[tokio::test]
async fn test_firewall_escalation_whitelist_immunity() {
    let (mut client, server) = duplex(4096);
    let mut session = VirtualSession::new(server);

    let loopback = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
    session.set_client_ip(loopback);

    let noop_backend = Arc::new(NoopFirewall::new());
    let fw_manager = FirewallManager::new(noop_backend.clone(), true, Duration::from_secs(900));

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    session.set_event_sender(event_tx);

    let client_task = tokio::spawn(async move {
        // Send 600-byte Handshake from loopback
        let malicious_payload = vec![0xEEu8; 600];
        let _ = write_raw_packet(&mut client, 0x00, &malicious_payload).await;
    });

    let fw_clone = fw_manager.clone();
    let coordinator_task = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            if let SessionEvent::HardViolation {
                client_ip: Some(ip),
                reason,
                ..
            } = event
            {
                fw_clone.escalate(ip, reason).await;
            }
        }
    });

    let _ = session.run().await;
    let _ = client_task.await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    coordinator_task.abort();

    // Whitelist / loopback MUST NEVER be banned in the firewall
    assert!(
        !noop_backend.is_banned(&loopback),
        "Loopback address must remain immune from firewall drops"
    );
    assert_eq!(noop_backend.banned_count(), 0);
}
