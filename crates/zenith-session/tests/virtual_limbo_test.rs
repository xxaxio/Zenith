use tokio::io::duplex;
use zenith_session::VirtualSession;
use zenith_wire::{
    Handshake, HandshakeIntent, KeepAlive, LoginStart, PROTOCOL_VERSION, read_raw_packet,
    write_raw_packet,
};

#[tokio::test]
async fn test_virtual_limbo_full_lifecycle() {
    let (client, server) = duplex(4096);
    let mut session = VirtualSession::new(server);

    let client_task = tokio::spawn(async move {
        let mut c = client;

        // 1. Handshake (Login intent)
        let hs = Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "localhost".into(),
            server_port: 25565,
            intent: HandshakeIntent::Login,
        };
        write_raw_packet(&mut c, 0x00, &hs.encode()).await.unwrap();

        // 2. Login Start
        let start = LoginStart {
            name: "Axxio".into(),
            uuid: 0xCAFE_BABE_0000_1111_2222_3333_4444_5555,
        };
        write_raw_packet(&mut c, 0x00, &start.encode())
            .await
            .unwrap();

        // 3. Receive Login Success (ID 0x02)
        let (success_id, _) = read_raw_packet(&mut c, 1024).await.unwrap();
        assert_eq!(success_id, 0x02);

        // 4. Send Login Acknowledged (ID 0x03)
        write_raw_packet(&mut c, 0x03, &[]).await.unwrap();

        // 5. Receive Finish Configuration from Server (ID 0x03)
        let (finish_cfg_id, _) = read_raw_packet(&mut c, 1024).await.unwrap();
        assert_eq!(finish_cfg_id, 0x03);

        // 6. Send Finish Configuration Acknowledgment (ID 0x03)
        write_raw_packet(&mut c, 0x03, &[]).await.unwrap();

        // 7. Receive Login (Play) packet (ID 50)
        let (login_play_id, _) = read_raw_packet(&mut c, 1024).await.unwrap();
        assert_eq!(login_play_id, 50);

        // 8. Consume Play initialization packets (GameEvent 39, ChunkBatchFinished 11, PlayerPosition 73, SystemChat 124) and reply to KeepAlive (45)
        loop {
            let (pkt_id, payload) = read_raw_packet(&mut c, 1024).await.unwrap();
            if pkt_id == 45 {
                let ka = KeepAlive::decode(&payload).unwrap();
                write_raw_packet(&mut c, 28, &ka.encode()).await.unwrap();
                break;
            }
        }
    });

    let server_task = tokio::spawn(async move {
        session.run().await.unwrap();
    });

    let (c_res, s_res) = tokio::join!(client_task, server_task);
    c_res.unwrap();
    s_res.unwrap();
}

#[tokio::test]
async fn test_virtual_limbo_resource_pack_and_broadcast_commands() {
    let (client, server) = duplex(4096);
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();

    let mut session = VirtualSession::with_command_channel(
        server,
        zenith_shield::ShieldLimits::default(),
        cmd_rx,
    );
    session.set_event_sender(event_tx);

    let client_task = tokio::spawn(async move {
        let mut c = client;

        // 1. Handshake & Login
        let hs = Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "localhost".into(),
            server_port: 25565,
            intent: HandshakeIntent::Login,
        };
        write_raw_packet(&mut c, 0x00, &hs.encode()).await.unwrap();

        let start = LoginStart {
            name: "Steve".into(),
            uuid: 0x1234_5678,
        };
        write_raw_packet(&mut c, 0x00, &start.encode())
            .await
            .unwrap();

        let (success_id, _) = read_raw_packet(&mut c, 1024).await.unwrap();
        assert_eq!(success_id, 0x02);
        write_raw_packet(&mut c, 0x03, &[]).await.unwrap(); // Login ack

        let (finish_cfg_id, _) = read_raw_packet(&mut c, 1024).await.unwrap();
        assert_eq!(finish_cfg_id, 0x03);
        write_raw_packet(&mut c, 0x03, &[]).await.unwrap(); // Config finish ack

        let (login_play_id, _) = read_raw_packet(&mut c, 1024).await.unwrap();
        assert_eq!(login_play_id, 50);

        // Read play packets until we get ResourcePackPush (82)
        let mut got_pack_push = false;
        let mut got_title_times = false;
        let mut got_title = false;
        let mut got_subtitle = false;
        let mut got_chat = false;

        while !(got_pack_push && got_title_times && got_title && got_subtitle && got_chat) {
            let (pkt_id, payload) = read_raw_packet(&mut c, 2048).await.unwrap();
            match pkt_id {
                82 => {
                    got_pack_push = true;
                    // Respond with Successfully Loaded (action = 0)
                    let resp = zenith_wire::ResourcePackResponse {
                        uuid: 0x9999,
                        action: zenith_wire::ResourcePackAction::SuccessfullyLoaded,
                    };
                    write_raw_packet(&mut c, 44, &resp.encode()).await.unwrap();
                }
                117 => got_title_times = true,
                116 => got_title = true,
                114 => got_subtitle = true,
                124 if String::from_utf8_lossy(&payload).contains("Emergency Maintenance") => {
                    got_chat = true;
                }
                _ => {}
            }
        }

        assert!(got_pack_push);
        assert!(got_title_times);
        assert!(got_title);
        assert!(got_subtitle);
        assert!(got_chat);
    });

    let sender_task = tokio::spawn(async move {
        // Send PushResourcePack command once client enters limbo
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        let _ = cmd_tx.send(zenith_session::SessionCommand::PushResourcePack {
            uuid: 0x9999,
            url: "https://example.com/pack.zip".into(),
            hash: "0123456789abcdef0123456789abcdef01234567".into(),
            required: false,
            prompt: Some("Preload queue textures".into()),
        });

        // Send BroadcastAlert command
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        let _ = cmd_tx.send(zenith_session::SessionCommand::BroadcastAlert {
            title: "§c§lALERT".into(),
            subtitle: Some("§7Server restart in 5m".into()),
            chat: "§c[Zenith Broadcast] §fEmergency Maintenance in 5m".into(),
        });

        // Close after test completes
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        let _ = cmd_tx.send(zenith_session::SessionCommand::Close);
    });

    let server_task = tokio::spawn(async move {
        session.run().await.unwrap();
    });

    let (c_res, s_res, send_res) = tokio::join!(client_task, server_task, sender_task);
    c_res.unwrap();
    s_res.unwrap();
    send_res.unwrap();

    // Verify event_rx received ResourcePackStatus
    let mut got_rp_event = false;
    while let Ok(event) = event_rx.try_recv() {
        if let zenith_session::SessionEvent::ResourcePackStatus { uuid, action, .. } = event {
            assert_eq!(uuid, 0x9999);
            assert_eq!(action, zenith_wire::ResourcePackAction::SuccessfullyLoaded);
            got_rp_event = true;
        }
    }
    assert!(
        got_rp_event,
        "Expected ResourcePackStatus event on session event channel"
    );
}
