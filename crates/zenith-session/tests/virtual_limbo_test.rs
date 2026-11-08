use tokio::io::{AsyncReadExt, duplex};
use zenith_session::VirtualSession;
use zenith_wire::{
    ClientInformation, CustomPayload, Handshake, HandshakeIntent, KeepAlive, LoginStart,
    PROTOCOL_VERSION, read_raw_packet, write_raw_packet, write_string,
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

        // 6. Send legitimate ClientInformation (0x00) and Brand (0x02)
        let info = ClientInformation::default();
        write_raw_packet(&mut c, 0x00, &info.encode())
            .await
            .unwrap();

        let mut brand_bytes = Vec::new();
        write_string("vanilla", &mut brand_bytes);
        let cp = CustomPayload {
            channel: "minecraft:brand".into(),
            data: brand_bytes,
        };
        write_raw_packet(&mut c, 0x02, &cp.encode()).await.unwrap();

        // 7. Send Finish Configuration Acknowledgment (ID 0x03)
        write_raw_packet(&mut c, 0x03, &[]).await.unwrap();

        // 8. Receive Login (Play) packet (ID 50)
        let (login_play_id, _) = read_raw_packet(&mut c, 1024).await.unwrap();
        assert_eq!(login_play_id, 50);

        // 9. Consume Play initialization packets (GameEvent 39, ChunkBatchFinished 11, PlayerPosition 73, SystemChat 124) and reply to KeepAlive (45)
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
async fn test_virtual_limbo_broadcast_alert_command() {
    let (client, server) = duplex(4096);
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let (event_tx, _event_rx) = tokio::sync::mpsc::unbounded_channel();

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

        // Send legitimate ClientInformation (0x00) and Brand (0x02)
        let info = ClientInformation::default();
        write_raw_packet(&mut c, 0x00, &info.encode())
            .await
            .unwrap();

        let mut brand_bytes = Vec::new();
        write_string("vanilla", &mut brand_bytes);
        let cp = CustomPayload {
            channel: "minecraft:brand".into(),
            data: brand_bytes,
        };
        write_raw_packet(&mut c, 0x02, &cp.encode()).await.unwrap();

        write_raw_packet(&mut c, 0x03, &[]).await.unwrap(); // Config finish ack

        let (login_play_id, _) = read_raw_packet(&mut c, 1024).await.unwrap();
        assert_eq!(login_play_id, 50);

        // Read play packets until we get TitleTimes, Title, Subtitle, Chat
        let mut got_title_times = false;
        let mut got_title = false;
        let mut got_subtitle = false;
        let mut got_chat = false;

        while !(got_title_times && got_title && got_subtitle && got_chat) {
            let (pkt_id, payload) = read_raw_packet(&mut c, 2048).await.unwrap();
            match pkt_id {
                117 => got_title_times = true,
                116 => got_title = true,
                114 => got_subtitle = true,
                124 if String::from_utf8_lossy(&payload).contains("Emergency Maintenance") => {
                    got_chat = true;
                }
                _ => {}
            }
        }

        assert!(got_title_times);
        assert!(got_title);
        assert!(got_subtitle);
        assert!(got_chat);
    });

    let sender_task = tokio::spawn(async move {
        // Send BroadcastAlert command once client enters limbo
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
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
}

#[tokio::test]
async fn test_virtual_limbo_rejects_bot_missing_client_info() {
    let (client, server) = duplex(4096);
    let mut session = VirtualSession::new(server);

    let client_task = tokio::spawn(async move {
        let mut c = client;

        // Handshake & Login
        let hs = Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "localhost".into(),
            server_port: 25565,
            intent: HandshakeIntent::Login,
        };
        write_raw_packet(&mut c, 0x00, &hs.encode()).await.unwrap();

        let start = LoginStart {
            name: "BotScript".into(),
            uuid: 0x9999_8888,
        };
        write_raw_packet(&mut c, 0x00, &start.encode())
            .await
            .unwrap();

        let (success_id, _) = read_raw_packet(&mut c, 1024).await.unwrap();
        assert_eq!(success_id, 0x02);
        write_raw_packet(&mut c, 0x03, &[]).await.unwrap(); // Login ack

        let (finish_cfg_id, _) = read_raw_packet(&mut c, 1024).await.unwrap();
        assert_eq!(finish_cfg_id, 0x03);

        // BOT BEHAVIOR: Deliberately skip ClientInformation and send 0x03 finish ack immediately
        write_raw_packet(&mut c, 0x03, &[]).await.unwrap();

        // Expect connection closed by server
        let mut check = [0u8; 1];
        let n = c.read(&mut check).await.unwrap_or(0);
        assert_eq!(
            n, 0,
            "Server must disconnect bot omitting ClientInformation"
        );
    });

    let server_task = tokio::spawn(async move {
        let res = session.run().await;
        assert!(
            res.is_err(),
            "Session must reject bot missing ClientInformation"
        );
    });

    let (c_res, s_res) = tokio::join!(client_task, server_task);
    c_res.unwrap();
    s_res.unwrap();
}

#[tokio::test]
async fn test_virtual_limbo_rejects_bot_with_blocked_brand() {
    let (client, server) = duplex(4096);
    let mut session = VirtualSession::new(server);

    let client_task = tokio::spawn(async move {
        let mut c = client;

        // Handshake & Login
        let hs = Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "localhost".into(),
            server_port: 25565,
            intent: HandshakeIntent::Login,
        };
        write_raw_packet(&mut c, 0x00, &hs.encode()).await.unwrap();

        let start = LoginStart {
            name: "AttackerPlayer".into(),
            uuid: 0x7777_6666,
        };
        write_raw_packet(&mut c, 0x00, &start.encode())
            .await
            .unwrap();

        let (success_id, _) = read_raw_packet(&mut c, 1024).await.unwrap();
        assert_eq!(success_id, 0x02);
        write_raw_packet(&mut c, 0x03, &[]).await.unwrap(); // Login ack

        let (finish_cfg_id, _) = read_raw_packet(&mut c, 1024).await.unwrap();
        assert_eq!(finish_cfg_id, 0x03);

        // Send valid ClientInformation
        let info = ClientInformation::default();
        write_raw_packet(&mut c, 0x00, &info.encode())
            .await
            .unwrap();

        // But send blocked brand "mcbot-v2"
        let mut brand_bytes = Vec::new();
        write_string("mcbot-v2", &mut brand_bytes);
        let cp = CustomPayload {
            channel: "minecraft:brand".into(),
            data: brand_bytes,
        };
        write_raw_packet(&mut c, 0x02, &cp.encode()).await.unwrap();

        // Send Finish Configuration Acknowledgment (ID 0x03)
        write_raw_packet(&mut c, 0x03, &[]).await.unwrap();

        // Expect connection closed by server
        let mut check = [0u8; 1];
        let n = c.read(&mut check).await.unwrap_or(0);
        assert_eq!(n, 0, "Server must disconnect bot sending blocked brand");
    });

    let server_task = tokio::spawn(async move {
        let res = session.run().await;
        assert!(
            res.is_err(),
            "Session must reject bot sending blocked brand"
        );
    });

    let (c_res, s_res) = tokio::join!(client_task, server_task);
    c_res.unwrap();
    s_res.unwrap();
}
