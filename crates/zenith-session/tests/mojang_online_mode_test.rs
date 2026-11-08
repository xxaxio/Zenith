//! Integration tests for Full Online Mojang Cryptography:
//! RSA 1024-bit key exchange, challenge nonce verification, AES-128 CFB8 stream cipher initialization,
//! and Mojang session server (`hasJoinedServer`) validation.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use zenith_session::{OnlineModeConfig, SessionState, VirtualSession};
use zenith_shield::ShieldLimits;
use zenith_wire::crypto::{
    EncryptedStream, EncryptionRequest, EncryptionResponse, RsaKeypair, generate_shared_secret,
    rsa_encrypt,
};
use zenith_wire::{
    Handshake, HandshakeIntent, LoginStart, PROTOCOL_VERSION, read_raw_packet, write_raw_packet,
};

/// Lightweight mock Mojang authentication HTTP service for integration testing.
struct MockMojangServer {
    pub addr: String,
    pub hits: Arc<AtomicUsize>,
}

impl MockMojangServer {
    pub async fn spawn(
        expected_username: &'static str,
        expected_uuid: &'static str,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let local_addr = listener.local_addr()?;
        let addr = format!("http://127.0.0.1:{}", local_addr.port());
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_clone = Arc::clone(&hits);

        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let hits_task = Arc::clone(&hits_clone);
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let n = match socket.read(&mut buf).await {
                        Ok(n) if n > 0 => n,
                        _ => return,
                    };
                    let req_str = String::from_utf8_lossy(&buf[..n]);
                    hits_task.fetch_add(1, Ordering::Relaxed);

                    // Check path and query
                    if req_str.contains(&format!("username={expected_username}")) {
                        let body = format!(
                            r#"{{"id":"{expected_uuid}","name":"{expected_username}","properties":[{{"name":"textures","value":"base64textures","signature":"sig123"}}]}}"#
                        );
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = socket.write_all(response.as_bytes()).await;
                    } else if req_str.contains("username=BadPlayer") {
                        // 204 No Content indicates user did not join via session server
                        let response = "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        let _ = socket.write_all(response.as_bytes()).await;
                    } else {
                        let response = "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        let _ = socket.write_all(response.as_bytes()).await;
                    }
                    let _ = socket.flush().await;
                });
            }
        });

        Ok(Self { addr, hits })
    }
}

#[tokio::test]
async fn test_online_mode_successful_handshake_and_crypto() {
    let mock_server = MockMojangServer::spawn("MojangTester", "5f3c1e6a2b9d4c7e8a1f0b2c3d4e5f60")
        .await
        .expect("failed to spawn mock server");

    let keypair = Arc::new(RsaKeypair::generate().expect("failed to generate server RSA key"));
    let (client_raw, server_raw) = tokio::io::duplex(8192);

    let client_stream = EncryptedStream::new(client_raw);
    let server_stream = EncryptedStream::new(server_raw);

    let mut session = VirtualSession::with_limits(server_stream, ShieldLimits::default());
    session.set_online_mode(OnlineModeConfig::Online {
        keypair: Arc::clone(&keypair),
        auth_endpoint: Some(mock_server.addr.clone()),
        prevent_proxy_connections: false,
    });

    let client_task = tokio::spawn(async move {
        let mut stream = client_stream;

        // 1. Send Handshake
        let hs = Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "play.horizon.dev".into(),
            server_port: 25565,
            intent: HandshakeIntent::Login,
        };
        write_raw_packet(&mut stream, 0x00, &hs.encode())
            .await
            .unwrap();

        // 2. Send LoginStart
        let start = LoginStart {
            name: "MojangTester".into(),
            uuid: 0,
        };
        write_raw_packet(&mut stream, 0x00, &start.encode())
            .await
            .unwrap();

        // 3. Receive EncryptionRequest (0x01)
        let (req_id, req_payload) = read_raw_packet(&mut stream, 2048).await.unwrap();
        assert_eq!(req_id, 0x01);
        let enc_req = EncryptionRequest::decode(&req_payload).unwrap();
        assert!(enc_req.should_authenticate);

        // 4. Generate random 16-byte shared secret
        let secret = generate_shared_secret().unwrap();

        // 5. Encrypt secret and challenge using server's RSA public key
        let encrypted_secret = rsa_encrypt(&enc_req.public_key, &secret).unwrap();
        let encrypted_challenge = rsa_encrypt(&enc_req.public_key, &enc_req.challenge).unwrap();

        // 6. Send EncryptionResponse (0x01)
        let enc_resp = EncryptionResponse {
            shared_secret: encrypted_secret,
            challenge: encrypted_challenge,
        };
        write_raw_packet(&mut stream, 0x01, &enc_resp.encode())
            .await
            .unwrap();

        // 7. Enable AES-128 CFB8 encryption on client stream!
        stream.enable_encryption(&secret);

        // 8. Read LoginSuccess (0x02) - transmitted encrypted over AES CFB8!
        let (succ_id, succ_payload) = read_raw_packet(&mut stream, 2048).await.unwrap();
        assert_eq!(succ_id, 0x02);
        assert!(!succ_payload.is_empty());

        // 9. Send LoginAcknowledged (0x03) - transmitted encrypted!
        write_raw_packet(&mut stream, 0x03, &[]).await.unwrap();

        // 10. Read FinishConfiguration (0x03) - encrypted!
        let (cfg_id, _) = read_raw_packet(&mut stream, 2048).await.unwrap();
        assert_eq!(cfg_id, 0x03);

        // 11. Send ClientInformation (0x00) and FinishConfiguration (0x03)
        let client_info = zenith_wire::ClientInformation::default();
        write_raw_packet(&mut stream, 0x00, &client_info.encode())
            .await
            .unwrap();
        write_raw_packet(&mut stream, 0x03, &[]).await.unwrap();

        // Ensure stream is properly encrypted
        assert!(stream.is_encrypted());
    });

    let server_task = tokio::spawn(async move {
        let res = session.complete_handshake_and_login().await;
        assert!(res.is_ok());
        let (name, uuid) = res.unwrap().expect("player should reach limbo");
        assert_eq!(name, "MojangTester");
        assert_eq!(uuid, 0x5f3c1e6a2b9d4c7e8a1f0b2c3d4e5f60);
        assert_eq!(session.state, SessionState::InLimbo);
    });

    tokio::time::timeout(Duration::from_secs(5), async {
        let (r1, r2) = tokio::join!(client_task, server_task);
        r1.unwrap();
        r2.unwrap();
    })
    .await
    .expect("test timed out");

    assert_eq!(mock_server.hits.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn test_online_mode_tampered_challenge_nonce_rejected() {
    let mock_server = MockMojangServer::spawn("MojangTester", "5f3c1e6a2b9d4c7e8a1f0b2c3d4e5f60")
        .await
        .expect("failed to spawn mock server");

    let keypair = Arc::new(RsaKeypair::generate().expect("failed to generate server RSA key"));
    let (client_raw, server_raw) = tokio::io::duplex(8192);

    let client_stream = EncryptedStream::new(client_raw);
    let server_stream = EncryptedStream::new(server_raw);

    let mut session = VirtualSession::with_limits(server_stream, ShieldLimits::default());
    session.set_online_mode(OnlineModeConfig::Online {
        keypair: Arc::clone(&keypair),
        auth_endpoint: Some(mock_server.addr.clone()),
        prevent_proxy_connections: false,
    });

    let client_task = tokio::spawn(async move {
        let mut stream = client_stream;

        // Handshake & LoginStart
        let hs = Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "play.horizon.dev".into(),
            server_port: 25565,
            intent: HandshakeIntent::Login,
        };
        write_raw_packet(&mut stream, 0x00, &hs.encode())
            .await
            .unwrap();

        let start = LoginStart {
            name: "MojangTester".into(),
            uuid: 0,
        };
        write_raw_packet(&mut stream, 0x00, &start.encode())
            .await
            .unwrap();

        // Receive EncryptionRequest (0x01)
        let (req_id, req_payload) = read_raw_packet(&mut stream, 2048).await.unwrap();
        assert_eq!(req_id, 0x01);
        let enc_req = EncryptionRequest::decode(&req_payload).unwrap();

        let secret = [0x55u8; 16];
        let corrupted_challenge = [0xFF, 0xEE, 0xDD, 0xCC];

        let encrypted_secret = rsa_encrypt(&enc_req.public_key, &secret).unwrap();
        let encrypted_challenge = rsa_encrypt(&enc_req.public_key, &corrupted_challenge).unwrap();

        // Send EncryptionResponse with corrupted challenge
        let enc_resp = EncryptionResponse {
            shared_secret: encrypted_secret,
            challenge: encrypted_challenge,
        };
        write_raw_packet(&mut stream, 0x01, &enc_resp.encode())
            .await
            .unwrap();
    });

    let server_task = tokio::spawn(async move {
        let res = session.complete_handshake_and_login().await;
        // Server MUST reject due to challenge mismatch
        assert!(res.is_err());
    });

    tokio::time::timeout(Duration::from_secs(5), async {
        let (r1, r2) = tokio::join!(client_task, server_task);
        r1.unwrap();
        r2.unwrap();
    })
    .await
    .expect("test timed out");
}

#[tokio::test]
async fn test_online_mode_mojang_unverified_player_rejected() {
    let mock_server = MockMojangServer::spawn("MojangTester", "5f3c1e6a2b9d4c7e8a1f0b2c3d4e5f60")
        .await
        .expect("failed to spawn mock server");

    let keypair = Arc::new(RsaKeypair::generate().expect("failed to generate server RSA key"));
    let (client_raw, server_raw) = tokio::io::duplex(8192);

    let client_stream = EncryptedStream::new(client_raw);
    let server_stream = EncryptedStream::new(server_raw);

    let mut session = VirtualSession::with_limits(server_stream, ShieldLimits::default());
    session.set_online_mode(OnlineModeConfig::Online {
        keypair: Arc::clone(&keypair),
        auth_endpoint: Some(mock_server.addr.clone()),
        prevent_proxy_connections: false,
    });

    let client_task = tokio::spawn(async move {
        let mut stream = client_stream;

        // Player "BadPlayer" (not verified in session server)
        let hs = Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "play.horizon.dev".into(),
            server_port: 25565,
            intent: HandshakeIntent::Login,
        };
        write_raw_packet(&mut stream, 0x00, &hs.encode())
            .await
            .unwrap();

        let start = LoginStart {
            name: "BadPlayer".into(),
            uuid: 0,
        };
        write_raw_packet(&mut stream, 0x00, &start.encode())
            .await
            .unwrap();

        let (req_id, req_payload) = read_raw_packet(&mut stream, 2048).await.unwrap();
        assert_eq!(req_id, 0x01);
        let enc_req = EncryptionRequest::decode(&req_payload).unwrap();

        let secret = [0x77u8; 16];
        let encrypted_secret = rsa_encrypt(&enc_req.public_key, &secret).unwrap();
        let encrypted_challenge = rsa_encrypt(&enc_req.public_key, &enc_req.challenge).unwrap();

        let enc_resp = EncryptionResponse {
            shared_secret: encrypted_secret,
            challenge: encrypted_challenge,
        };
        write_raw_packet(&mut stream, 0x01, &enc_resp.encode())
            .await
            .unwrap();

        stream.enable_encryption(&secret);
    });

    let server_task = tokio::spawn(async move {
        let res = session.complete_handshake_and_login().await;
        // Server MUST reject since Mojang returned 204 No Content
        assert!(res.is_err());
    });

    tokio::time::timeout(Duration::from_secs(5), async {
        let (r1, r2) = tokio::join!(client_task, server_task);
        r1.unwrap();
        r2.unwrap();
    })
    .await
    .expect("test timed out");

    assert_eq!(mock_server.hits.load(Ordering::Relaxed), 1);
}
