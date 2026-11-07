//! Mock Horizon Dedicated Server backend for SLP probing, transfer landing, and #7484 Turbo IPC.

use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};
use zenith_wire::{
    Handshake, HandshakeIntent, LoginSuccess, PROTOCOL_VERSION, read_raw_packet, write_raw_packet,
    write_string,
};

/// Live state and telemetry tracked by the Mock Horizon Backend.
#[derive(Debug, Default)]
pub struct MockBackendStats {
    pub slp_requests: AtomicUsize,
    pub transfers_completed: AtomicUsize,
    pub turbo_queries: AtomicUsize,
    pub turbo_expects: AtomicUsize,
}

/// A simulated Horizon Dedicated Server.
pub struct MockHorizonBackend {
    pub bind_addr: SocketAddr,
    pub turbo_addr: Option<SocketAddr>,
    pub is_online: Arc<AtomicBool>,
    pub stats: Arc<MockBackendStats>,
    shutdown_tx: broadcast::Sender<()>,
}

impl MockHorizonBackend {
    /// Binds the mock Minecraft port (e.g. `127.0.0.1:25566`) and optional Turbo control port.
    pub async fn bind(
        mc_addr: SocketAddr,
        turbo_addr: Option<SocketAddr>,
    ) -> Result<Self, std::io::Error> {
        let (shutdown_tx, _) = broadcast::channel(1);
        let is_online = Arc::new(AtomicBool::new(true));
        let stats = Arc::new(MockBackendStats::default());

        let mc_listener = TcpListener::bind(mc_addr).await?;
        let bound_mc_addr = mc_listener.local_addr()?;
        info!(addr = %bound_mc_addr, "Mock Horizon Minecraft listener ready");

        // Spawn Minecraft / SLP / Transfer connection handler
        let online_flag = Arc::clone(&is_online);
        let mc_stats = Arc::clone(&stats);
        let mut mc_shutdown = shutdown_tx.subscribe();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    accept_res = mc_listener.accept() => {
                        match accept_res {
                            Ok((stream, peer)) => {
                                if !online_flag.load(Ordering::Relaxed) {
                                    debug!(%peer, "Mock backend offline: dropping connection");
                                    drop(stream);
                                    continue;
                                }
                                let conn_online = Arc::clone(&online_flag);
                                let conn_stats = Arc::clone(&mc_stats);
                                tokio::spawn(async move {
                                    if let Err(e) = handle_mc_connection(stream, conn_online, conn_stats).await {
                                        debug!(%peer, error = %e, "Mock client connection ended");
                                    }
                                });
                            }
                            Err(e) => {
                                warn!(error = %e, "Mock MC listener accept error");
                                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                            }
                        }
                    }
                    _ = mc_shutdown.recv() => {
                        debug!("Shutting down Mock MC listener");
                        break;
                    }
                }
            }
        });

        // Spawn optional Turbo #7484 control socket handler
        let bound_turbo_addr = if let Some(t_addr) = turbo_addr {
            let turbo_listener = TcpListener::bind(t_addr).await?;
            let bound_t_addr = turbo_listener.local_addr()?;
            info!(addr = %bound_t_addr, "Mock Horizon Turbo control listener ready");

            let turbo_online = Arc::clone(&is_online);
            let turbo_stats = Arc::clone(&stats);
            let mut turbo_shutdown = shutdown_tx.subscribe();

            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        accept_res = turbo_listener.accept() => {
                            if let Ok((stream, _)) = accept_res {
                                if !turbo_online.load(Ordering::Relaxed) {
                                    drop(stream);
                                    continue;
                                }
                                let t_stats = Arc::clone(&turbo_stats);
                                tokio::spawn(async move {
                                    let _ = handle_turbo_connection(stream, t_stats).await;
                                });
                            }
                        }
                        _ = turbo_shutdown.recv() => {
                            debug!("Shutting down Mock Turbo listener");
                            break;
                        }
                    }
                }
            });

            Some(bound_t_addr)
        } else {
            None
        };

        Ok(Self {
            bind_addr: bound_mc_addr,
            turbo_addr: bound_turbo_addr,
            is_online,
            stats,
            shutdown_tx,
        })
    }

    /// Toggles the online/offline state of the mock server.
    pub fn set_online(&self, online: bool) {
        self.is_online.store(online, Ordering::SeqCst);
    }

    /// Shuts down all listener loops.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(());
    }
}

/// Handles incoming connections to the Minecraft port (SLP or Player Transfer).
async fn handle_mc_connection(
    mut stream: TcpStream,
    is_online: Arc<AtomicBool>,
    stats: Arc<MockBackendStats>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 1. Read Handshake packet
    let (packet_id, payload) = read_raw_packet(&mut stream, 1024).await?;
    if packet_id != 0x00 {
        return Ok(());
    }

    let handshake = Handshake::decode(&payload)?;
    match handshake.intent {
        HandshakeIntent::Status => {
            // SLP flow
            stats.slp_requests.fetch_add(1, Ordering::Relaxed);
            let (req_id, _) = read_raw_packet(&mut stream, 64).await?;
            if req_id != 0x00 {
                return Ok(());
            }

            let status_json = json!({
                "version": {
                    "name": "Horizon 26.3",
                    "protocol": PROTOCOL_VERSION
                },
                "players": {
                    "max": 1000,
                    "online": stats.transfers_completed.load(Ordering::Relaxed)
                },
                "description": {
                    "text": "Mock Horizon Main World"
                }
            })
            .to_string();

            let mut body = Vec::new();
            write_string(&status_json, &mut body);
            write_raw_packet(&mut stream, 0x00, &body).await?;

            // Optional Ping/Pong
            if let Ok((ping_id, ping_payload)) = read_raw_packet(&mut stream, 64).await {
                if ping_id == 0x01 {
                    write_raw_packet(&mut stream, 0x01, &ping_payload).await?;
                }
            }
        }
        HandshakeIntent::Login | HandshakeIntent::Transfer => {
            // Player arriving via 0x7C Transfer!
            let (login_id, _) = read_raw_packet(&mut stream, 1024).await?;
            if login_id != 0x00 {
                return Ok(());
            }

            // Respond with LoginSuccess (0x02)
            let success = LoginSuccess {
                uuid: 0x1234_5678_9abc_def0_1234_5678_9abc_def0,
                username: "TransferredPlayer".to_string(),
            };
            write_raw_packet(&mut stream, 0x02, &success.encode()).await?;

            stats.transfers_completed.fetch_add(1, Ordering::Relaxed);
            debug!("Mock backend successfully accepted transferred player!");

            // Hold connection alive until client disconnects or backend goes offline
            let mut dummy_buf = [0u8; 128];
            while is_online.load(Ordering::Relaxed) {
                tokio::select! {
                    n = stream.read(&mut dummy_buf) => {
                        if n.unwrap_or(0) == 0 {
                            break;
                        }
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
                }
            }
        }
    }

    Ok(())
}

/// Handles incoming commands on the Horizon Turbo #7484 control socket.
async fn handle_turbo_connection(
    stream: TcpStream,
    stats: Arc<MockBackendStats>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();

    if reader.read_line(&mut line).await? == 0 {
        return Ok(());
    }

    let trimmed = line.trim();
    if trimmed == "status" {
        stats.turbo_queries.fetch_add(1, Ordering::Relaxed);
        let resp = json!({
            "mspt_us": 2400,
            "tps": 20.0,
            "online_players": stats.transfers_completed.load(Ordering::Relaxed),
            "max_players": 1000,
            "dimensions": 3
        })
        .to_string();

        let mut stream = reader.into_inner();
        stream.write_all(format!("{resp}\n").as_bytes()).await?;
        stream.flush().await?;
    } else if trimmed.starts_with("player-expect") {
        stats.turbo_expects.fetch_add(1, Ordering::Relaxed);
        let mut stream = reader.into_inner();
        stream.write_all(b"ok\n").await?;
        stream.flush().await?;
    } else if trimmed.starts_with("player-confirm") {
        let mut stream = reader.into_inner();
        stream.write_all(b"ok\n").await?;
        stream.flush().await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_backend_slp() {
        let backend = MockHorizonBackend::bind("127.0.0.1:0".parse().unwrap(), None)
            .await
            .unwrap();

        let mut client = TcpStream::connect(backend.bind_addr).await.unwrap();

        // Send Handshake intent 1 (Status)
        let hs = Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "127.0.0.1".into(),
            server_port: backend.bind_addr.port(),
            intent: HandshakeIntent::Status,
        };
        let hs_body = hs.encode();
        write_raw_packet(&mut client, 0x00, &hs_body).await.unwrap();

        // Send StatusRequest (0x00)
        write_raw_packet(&mut client, 0x00, &[]).await.unwrap();

        // Read StatusResponse
        let (id, payload) = read_raw_packet(&mut client, 4096).await.unwrap();
        assert_eq!(id, 0x00);

        let mut cur = &payload[..];
        let status_str = zenith_wire::read_string(&mut cur, 4096).unwrap();
        assert!(status_str.contains("Mock Horizon Main World"));

        assert_eq!(backend.stats.slp_requests.load(Ordering::Relaxed), 1);
        backend.shutdown();
    }
}
