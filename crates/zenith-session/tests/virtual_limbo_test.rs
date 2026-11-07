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
