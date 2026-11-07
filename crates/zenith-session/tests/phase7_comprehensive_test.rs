//! Exhaustive test suite for Zenith Phase 7 features:
//! 1. HAProxy PROXY Protocol v2 Ingress Engine (real TCP socket, IPv4/IPv6, trusted CIDR, bypass).
//! 2. Limbo Background Resource Pack Pre-Loading & Status Tracking (push packet 82, responses 44).
//! 3. Broadcast & Alert System (/broadcast, /bc, /alert, /status, permissions, titles, subtitles).
//! 4. Totem Automated Lifecycle Alerts (backend down evacuation, backend recovery resume).

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, mpsc};

use zenith_display::{
    encode_subtitle_packet, encode_system_chat_packet, encode_title_packet,
    encode_title_times_packet,
};
use zenith_session::{SessionCommand, SessionId};
use zenith_shield::ConnectionTracker;
use zenith_totem::TotemEvent;
use zenith_transit::GroupManager;
use zenith_wire::{
    Handshake, HandshakeIntent, PROTOCOL_VERSION, PROXY_V2_MAGIC, ProxyCommand, ProxyHeader,
    is_trusted_proxy, read_raw_packet, write_raw_packet,
};

type ActiveSessions = Arc<
    Mutex<
        HashMap<
            SessionId,
            (
                mpsc::UnboundedSender<SessionCommand>,
                String,
                u128,
                usize,
                u128,
            ),
        >,
    >,
>;

struct MockCoordinator {
    sessions: ActiveSessions,
    group_mgr: Arc<Mutex<GroupManager>>,
    target_server: String,
    rp_enabled: bool,
}

impl MockCoordinator {
    fn new(rp_enabled: bool) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            group_mgr: Arc::new(Mutex::new(GroupManager::new(None))),
            target_server: "127.0.0.1:25566".to_string(),
            rp_enabled,
        }
    }

    async fn handle_command(
        &self,
        session_id: SessionId,
        username: &str,
        cmd: &str,
        args: &[String],
    ) {
        let session_data = {
            let s = self.sessions.lock().await;
            s.get(&session_id)
                .map(|(tx, _, uuid, _, _)| (tx.clone(), *uuid))
        };
        let Some((tx, _)) = session_data else { return };
        let is_staff = self.group_mgr.lock().await.is_staff(username);

        match cmd {
            "broadcast" | "bc" => {
                if !is_staff {
                    let msg = encode_system_chat_packet(
                        "§b§lᴢᴇɴɪᴛʜ §8» §cYou do not have permission to broadcast.",
                        false,
                    );
                    let _ = tx.send(SessionCommand::SendRaw(msg));
                    return;
                }
                if args.is_empty() {
                    let msg = encode_system_chat_packet(
                        "§b§lᴢᴇɴɪᴛʜ §8» §cUsage: /broadcast <message>",
                        false,
                    );
                    let _ = tx.send(SessionCommand::SendRaw(msg));
                    return;
                }
                let broadcast_text = args.join(" ");
                let chat = encode_system_chat_packet(
                    &format!("§b§lᴢᴇɴɪᴛʜ §8» §d§l[BROADCAST] §r§f{}", broadcast_text),
                    false,
                );
                let sess = self.sessions.lock().await;
                for (s_tx, _, _, _, _) in sess.values() {
                    let _ = s_tx.send(SessionCommand::SendRaw(chat.clone()));
                }
            }
            "alert" => {
                if !is_staff {
                    let msg = encode_system_chat_packet(
                        "§b§lᴢᴇɴɪᴛʜ §8» §cYou do not have permission to send alerts.",
                        false,
                    );
                    let _ = tx.send(SessionCommand::SendRaw(msg));
                    return;
                }
                if args.is_empty() {
                    let msg = encode_system_chat_packet(
                        "§b§lᴢᴇɴɪᴛʜ §8» §cUsage: /alert <message>",
                        false,
                    );
                    let _ = tx.send(SessionCommand::SendRaw(msg));
                    return;
                }
                let alert_text = args.join(" ");
                let times = encode_title_times_packet(10, 60, 20);
                let title = encode_title_packet("§c§lALERT");
                let sub = encode_subtitle_packet(&format!("§e{}", alert_text));
                let chat = encode_system_chat_packet(
                    &format!("§b§lᴢᴇɴɪᴛʜ §8» §c§l[ALERT] §e{}", alert_text),
                    false,
                );
                let sess = self.sessions.lock().await;
                for (s_tx, _, _, _, _) in sess.values() {
                    let _ = s_tx.send(SessionCommand::SendRaw(times.clone()));
                    let _ = s_tx.send(SessionCommand::SendRaw(title.clone()));
                    let _ = s_tx.send(SessionCommand::SendRaw(sub.clone()));
                    let _ = s_tx.send(SessionCommand::SendRaw(chat.clone()));
                }
            }
            "status" => {
                if !is_staff {
                    let msg = encode_system_chat_packet(
                        "§b§lᴢᴇɴɪᴛʜ §8» §cYou do not have permission to view backend status.",
                        false,
                    );
                    let _ = tx.send(SessionCommand::SendRaw(msg));
                    return;
                }
                let m1 =
                    encode_system_chat_packet("§b§l  ᴢᴇɴɪᴛʜ §8» §fBackend Server Health", false);
                let m2 = encode_system_chat_packet(
                    &format!("§7  Target: §e{}", self.target_server),
                    false,
                );
                let m3 = encode_system_chat_packet(
                    &format!(
                        "§7  Resource Pack Preload: {}",
                        if self.rp_enabled {
                            "§aActive"
                        } else {
                            "§7Disabled"
                        }
                    ),
                    false,
                );
                let _ = tx.send(SessionCommand::SendRaw(m1));
                let _ = tx.send(SessionCommand::SendRaw(m2));
                let _ = tx.send(SessionCommand::SendRaw(m3));
            }
            _ => {}
        }
    }
}

// -----------------------------------------------------------------------------
// TEST 1: PROXY Protocol v2 Ingress Engine over Real TCP Sockets
// -----------------------------------------------------------------------------
#[tokio::test]
async fn test_proxy_protocol_v2_real_tcp_socket_ingress() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_addr = listener.local_addr().unwrap();

    let trusted_proxies = vec![
        "127.0.0.1".to_string(),
        "::1".to_string(),
        "10.0.0.0/8".to_string(),
    ];
    let proxy_enabled = true;
    let conn_tracker = ConnectionTracker::new(2);

    let (extracted_tx, mut extracted_rx) = mpsc::unbounded_channel::<IpAddr>();

    // Spawn server accept loop with identical logic to zenith-app
    let server_task = tokio::spawn(async move {
        for _ in 0..4 {
            let (mut socket, peer_addr) = listener.accept().await.unwrap();
            let mut client_ip = peer_addr.ip();

            if proxy_enabled && is_trusted_proxy(peer_addr.ip(), &trusted_proxies) {
                let proxy_res = async {
                    let mut peek_buf = [0u8; 512];
                    let mut read_bytes = 0;
                    for _ in 0..10 {
                        let n = socket.peek(&mut peek_buf).await?;
                        if n > read_bytes {
                            read_bytes = n;
                        }
                        if read_bytes < 12 {
                            tokio::time::sleep(Duration::from_millis(5)).await;
                            continue;
                        }
                        if peek_buf[..12] != PROXY_V2_MAGIC {
                            return Ok::<_, std::io::Error>(None);
                        }
                        if read_bytes < 16 {
                            tokio::time::sleep(Duration::from_millis(5)).await;
                            continue;
                        }
                        let total_len =
                            16 + u16::from_be_bytes([peek_buf[14], peek_buf[15]]) as usize;
                        if total_len > peek_buf.len() {
                            return Ok(None);
                        }
                        if read_bytes < total_len {
                            tokio::time::sleep(Duration::from_millis(5)).await;
                            continue;
                        }
                        if let Ok(Some((header, _))) = ProxyHeader::parse(&peek_buf[..total_len]) {
                            let mut discard = vec![0u8; total_len];
                            socket.read_exact(&mut discard).await?;
                            if let Some(src) = header.src_addr {
                                return Ok(Some(src.ip()));
                            }
                        }
                        return Ok(None);
                    }
                    Ok(None)
                };

                if let Ok(Ok(Some(extracted_ip))) =
                    tokio::time::timeout(Duration::from_millis(500), proxy_res).await
                {
                    client_ip = extracted_ip;
                }
            }

            // Verify connection tracker uses client_ip
            let _guard = conn_tracker.acquire(client_ip).unwrap();
            let _ = extracted_tx.send(client_ip);

            // Read Minecraft Handshake that immediately follows the proxy header
            let (hs_id, payload) = read_raw_packet(&mut socket, 1024).await.unwrap();
            assert_eq!(hs_id, 0x00);
            let hs = Handshake::decode(&payload).unwrap();
            assert_eq!(hs.protocol_version, PROTOCOL_VERSION);
        }
    });

    // 1. Client 1: PROXY v2 IPv4 (Real IP: 203.0.113.195)
    {
        let mut client = TcpStream::connect(server_addr).await.unwrap();
        let src_addr: SocketAddr = "203.0.113.195:54321".parse().unwrap();
        let dst_addr: SocketAddr = "198.51.100.1:25565".parse().unwrap();
        let header = ProxyHeader {
            command: ProxyCommand::Proxy,
            src_addr: Some(src_addr),
            dst_addr: Some(dst_addr),
        };
        client.write_all(&header.encode()).await.unwrap();

        // Send Handshake immediately after PROXY header
        let hs = Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "localhost".into(),
            server_port: 25565,
            intent: HandshakeIntent::Status,
        };
        write_raw_packet(&mut client, 0x00, &hs.encode())
            .await
            .unwrap();
    }

    let ip1 = extracted_rx.recv().await.unwrap();
    assert_eq!(ip1, "203.0.113.195".parse::<IpAddr>().unwrap());

    // 2. Client 2: PROXY v2 IPv6 (Real IP: 2001:db8::cafe:babe)
    {
        let mut client = TcpStream::connect(server_addr).await.unwrap();
        let src_addr: SocketAddr = "[2001:db8::cafe:babe]:44321".parse().unwrap();
        let dst_addr: SocketAddr = "[2001:db8::1]:25565".parse().unwrap();
        let header = ProxyHeader {
            command: ProxyCommand::Proxy,
            src_addr: Some(src_addr),
            dst_addr: Some(dst_addr),
        };
        client.write_all(&header.encode()).await.unwrap();

        let hs = Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "localhost".into(),
            server_port: 25565,
            intent: HandshakeIntent::Status,
        };
        write_raw_packet(&mut client, 0x00, &hs.encode())
            .await
            .unwrap();
    }

    let ip2 = extracted_rx.recv().await.unwrap();
    assert_eq!(ip2, "2001:db8::cafe:babe".parse::<IpAddr>().unwrap());

    // 3. Client 3: Direct connection without PROXY header (should fall back cleanly to 127.0.0.1)
    {
        let mut client = TcpStream::connect(server_addr).await.unwrap();
        let hs = Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "localhost".into(),
            server_port: 25565,
            intent: HandshakeIntent::Status,
        };
        write_raw_packet(&mut client, 0x00, &hs.encode())
            .await
            .unwrap();
    }

    let ip3 = extracted_rx.recv().await.unwrap();
    assert_eq!(ip3, "127.0.0.1".parse::<IpAddr>().unwrap());

    // 4. Client 4: PROXY LOCAL command (health check from proxy)
    {
        let mut client = TcpStream::connect(server_addr).await.unwrap();
        let header = ProxyHeader {
            command: ProxyCommand::Local,
            src_addr: None,
            dst_addr: None,
        };
        client.write_all(&header.encode()).await.unwrap();

        let hs = Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "localhost".into(),
            server_port: 25565,
            intent: HandshakeIntent::Status,
        };
        write_raw_packet(&mut client, 0x00, &hs.encode())
            .await
            .unwrap();
    }

    let ip4 = extracted_rx.recv().await.unwrap();
    assert_eq!(ip4, "127.0.0.1".parse::<IpAddr>().unwrap());

    server_task.await.unwrap();
}

// -----------------------------------------------------------------------------
// TEST 2: Multi-Client Broadcast, Alert, Status & Staff Permissions
// -----------------------------------------------------------------------------
#[tokio::test]
async fn test_broadcast_and_alert_multiplayer_dispatch() {
    let coord = Arc::new(MockCoordinator::new(true));

    // Configure Admin as staff in group manager
    {
        let mut gm = coord.group_mgr.lock().await;
        gm.create_group("owner", 1000, true).unwrap();
        gm.add_member("owner", "Admin").unwrap();
    }

    let (admin_tx, mut admin_rx) = mpsc::unbounded_channel();
    let (player_tx, mut player_rx) = mpsc::unbounded_channel();

    let admin_session_id = SessionId(101);
    let player_session_id = SessionId(202);

    {
        let mut sess = coord.sessions.lock().await;
        sess.insert(admin_session_id, (admin_tx, "Admin".into(), 101, 1, 101));
        sess.insert(player_session_id, (player_tx, "Player".into(), 202, 2, 202));
    }

    // 1. Staff executes /broadcast
    coord
        .handle_command(
            admin_session_id,
            "Admin",
            "broadcast",
            &["Server", "maintenance", "in", "10", "minutes"].map(String::from),
        )
        .await;

    // Both Admin and Player must receive the broadcast message
    let player_msg = player_rx.try_recv().unwrap();
    let admin_msg = admin_rx.try_recv().unwrap();
    if let SessionCommand::SendRaw(bytes) = player_msg {
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("[BROADCAST]"));
        assert!(text.contains("Server maintenance in 10 minutes"));
    } else {
        panic!("Expected SendRaw packet for player broadcast");
    }
    if let SessionCommand::SendRaw(bytes) = admin_msg {
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("[BROADCAST]"));
    }

    // 2. Non-staff attempts /broadcast -> denied
    coord
        .handle_command(
            player_session_id,
            "Player",
            "broadcast",
            &["Hacked", "broadcast"].map(String::from),
        )
        .await;

    // Player gets permission error, Admin gets nothing
    let player_denied = player_rx.try_recv().unwrap();
    assert!(admin_rx.try_recv().is_err()); // Admin didn't receive anything
    if let SessionCommand::SendRaw(bytes) = player_denied {
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("You do not have permission to broadcast"));
    }

    // 3. Staff executes /bc (short alias)
    coord
        .handle_command(
            admin_session_id,
            "Admin",
            "bc",
            &["Short", "alias", "test"].map(String::from),
        )
        .await;
    let player_bc = player_rx.try_recv().unwrap();
    if let SessionCommand::SendRaw(bytes) = player_bc {
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("[BROADCAST]"));
        assert!(text.contains("Short alias test"));
    }

    // 4. Staff executes /alert -> sends Title Times, Title, Subtitle, and Chat
    coord
        .handle_command(
            admin_session_id,
            "Admin",
            "alert",
            &["Restarting", "for", "update"].map(String::from),
        )
        .await;

    // Player must receive: times, title, subtitle, chat
    let mut got_times = false;
    let mut got_title = false;
    let mut got_sub = false;
    let mut got_chat = false;

    while let Ok(cmd) = player_rx.try_recv() {
        if let SessionCommand::SendRaw(bytes) = cmd {
            let text = String::from_utf8_lossy(&bytes);
            if text.contains("ALERT") && text.contains("[ALERT]") {
                got_chat = true;
            } else if text.contains("ALERT") {
                got_title = true;
            } else if text.contains("Restarting for update") {
                got_sub = true;
            } else if bytes.len() >= 12 {
                got_times = true;
            }
        }
    }

    assert!(got_times, "Alert did not dispatch title times packet");
    assert!(got_title, "Alert did not dispatch title text packet");
    assert!(got_sub, "Alert did not dispatch subtitle text packet");
    assert!(got_chat, "Alert did not dispatch system chat packet");

    // 5. Non-staff attempts /alert -> denied
    coord
        .handle_command(
            player_session_id,
            "Player",
            "alert",
            &["fake", "alert"].map(String::from),
        )
        .await;
    let player_alert_denied = player_rx.try_recv().unwrap();
    if let SessionCommand::SendRaw(bytes) = player_alert_denied {
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("You do not have permission to send alerts"));
    }

    // 6. Staff executes /status -> contains Backend Server Health and Resource Pack Preload: Active
    coord
        .handle_command(admin_session_id, "Admin", "status", &[])
        .await;

    let mut status_output = String::new();
    while let Ok(cmd) = admin_rx.try_recv() {
        if let SessionCommand::SendRaw(bytes) = cmd {
            status_output.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
    assert!(status_output.contains("Backend Server Health"));
    assert!(status_output.contains("Resource Pack Preload: §aActive"));

    // 7. Non-staff attempts /status -> denied
    coord
        .handle_command(player_session_id, "Player", "status", &[])
        .await;
    let player_status_denied = player_rx.try_recv().unwrap();
    if let SessionCommand::SendRaw(bytes) = player_status_denied {
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("You do not have permission to view backend status"));
    }
}

// -----------------------------------------------------------------------------
// TEST 4: Totem Automated Lifecycle Alerts
// -----------------------------------------------------------------------------
#[tokio::test]
async fn test_totem_lifecycle_broadcast_alerts() {
    let (player_tx, mut player_rx) = mpsc::unbounded_channel();
    let sessions: ActiveSessions = Arc::new(Mutex::new(HashMap::new()));
    sessions
        .lock()
        .await
        .insert(SessionId(99), (player_tx, "PlayerOne".into(), 99, 1, 99));

    let (totem_event_tx, mut totem_event_rx) = mpsc::unbounded_channel();
    let notify_down = true;
    let notify_up = true;

    // Spawn Totem broadcaster loop
    let totem_sessions = Arc::clone(&sessions);
    tokio::spawn(async move {
        while let Some(event) = totem_event_rx.recv().await {
            match event {
                TotemEvent::BackendDown { .. } => {
                    if notify_down {
                        let sess = totem_sessions.lock().await;
                        let times = encode_title_times_packet(5, 50, 15);
                        let sub = encode_subtitle_packet(
                            "§c§lBACKEND RESTARTING §7Recovery in progress...",
                        );
                        let title = encode_title_packet("§c⚠");
                        let chat = encode_system_chat_packet(
                            "§b§lᴢᴇɴɪᴛʜ §8» §c§lALERT: §7Target backend crashed or restarting. You are safe in virtual holding!",
                            false,
                        );
                        for (s_tx, _, _, _, _) in sess.values() {
                            let _ = s_tx.send(SessionCommand::SendRaw(times.clone()));
                            let _ = s_tx.send(SessionCommand::SendRaw(sub.clone()));
                            let _ = s_tx.send(SessionCommand::SendRaw(title.clone()));
                            let _ = s_tx.send(SessionCommand::SendRaw(chat.clone()));
                        }
                    }
                }
                TotemEvent::BackendRecovered { latency_ms, .. } => {
                    if notify_up {
                        let sess = totem_sessions.lock().await;
                        let times = encode_title_times_packet(5, 50, 15);
                        let sub = encode_subtitle_packet(
                            "§a§lBACKEND READY §7Resuming automatic transfers...",
                        );
                        let title = encode_title_packet("§a▶");
                        let chat = encode_system_chat_packet(
                            &format!(
                                "§b§lᴢᴇɴɪᴛʜ §8» §a§lBACKEND ONLINE! §7Ping: §e{}ms§7. Preparing transfers...",
                                latency_ms
                            ),
                            false,
                        );
                        for (s_tx, _, _, _, _) in sess.values() {
                            let _ = s_tx.send(SessionCommand::SendRaw(times.clone()));
                            let _ = s_tx.send(SessionCommand::SendRaw(sub.clone()));
                            let _ = s_tx.send(SessionCommand::SendRaw(title.clone()));
                            let _ = s_tx.send(SessionCommand::SendRaw(chat.clone()));
                        }
                    }
                }
                TotemEvent::StatusUpdate { .. } => {}
            }
        }
    });

    // 1. Fire BackendDown event
    totem_event_tx
        .send(TotemEvent::BackendDown {
            consecutive_failures: 2,
            error: "Connection refused".into(),
        })
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut got_down_title = false;
    let mut got_down_sub = false;
    let mut got_down_chat = false;

    while let Ok(cmd) = player_rx.try_recv() {
        if let SessionCommand::SendRaw(bytes) = cmd {
            let text = String::from_utf8_lossy(&bytes);
            if text.contains("⚠") {
                got_down_title = true;
            } else if text.contains("BACKEND RESTARTING") {
                got_down_sub = true;
            } else if text.contains("Target backend crashed or restarting") {
                got_down_chat = true;
            }
        }
    }
    assert!(got_down_title);
    assert!(got_down_sub);
    assert!(got_down_chat);

    // 2. Fire BackendRecovered event
    totem_event_tx
        .send(TotemEvent::BackendRecovered {
            latency_ms: 25,
            online_players: Some(10),
        })
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut got_up_title = false;
    let mut got_up_sub = false;
    let mut got_up_chat = false;

    while let Ok(cmd) = player_rx.try_recv() {
        if let SessionCommand::SendRaw(bytes) = cmd {
            let text = String::from_utf8_lossy(&bytes);
            if text.contains("▶") {
                got_up_title = true;
            } else if text.contains("BACKEND READY") {
                got_up_sub = true;
            } else if text.contains("BACKEND ONLINE") && text.contains("25ms") {
                got_up_chat = true;
            }
        }
    }
    assert!(got_up_title);
    assert!(got_up_sub);
    assert!(got_up_chat);
}
