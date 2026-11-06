//! Zero-chunk virtual dimension session holding and lightweight connection actors.

use bytes::{BufMut, BytesMut};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::{debug, info};
use zenith_shield::{ShieldError, ShieldLimits, ShieldedStream};
use zenith_wire::{
    Handshake, HandshakeIntent, KeepAlive, LoginStart, LoginSuccess, PROTOCOL_VERSION, WireError,
    write_string, write_varint,
};

/// Unique identifier for an active player session in Zenith.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(pub u64);

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

impl SessionId {
    /// Generates a new unique session identifier.
    #[must_use]
    pub fn next() -> Self {
        Self(NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// Lifecycle state of a player session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Handshake,
    Status,
    Login,
    Configuration,
    InLimbo,
    Terminated,
}

/// Session processing errors.
#[derive(Debug, Error)]
pub enum SessionError {
    #[error("Shield error: {0}")]
    Shield(#[from] ShieldError),
    #[error("Wire protocol error: {0}")]
    Wire(#[from] WireError),
    #[error("Unexpected packet in state {state:?}: ID 0x{id:02X}")]
    UnexpectedPacket { state: SessionState, id: i32 },
    #[error("Connection closed by peer")]
    ConnectionClosed,
}

/// Commands that can be dispatched to an active virtual session actor.
#[derive(Debug, Clone)]
pub enum SessionCommand {
    /// Send pre-encoded raw packet bytes down the connection.
    SendRaw(Vec<u8>),
    /// Dispatches the Protocol 777 Transfer packet (ID 0x7C / 124) and closes the session.
    Transfer { host: String, port: u16 },
    /// Terminates the session cleanly.
    Close,
}

/// A lightweight, zero-chunk virtual session actor holding a connected player.
pub struct VirtualSession<S> {
    pub id: SessionId,
    pub state: SessionState,
    pub username: Option<String>,
    pub uuid: Option<u128>,
    pub connected_at: Instant,
    pub last_ping: Option<Instant>,
    pub ping_ms: u32,
    shield: ShieldedStream<S>,
    pub cmd_rx: Option<tokio::sync::mpsc::UnboundedReceiver<SessionCommand>>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> VirtualSession<S> {
    /// Spawns a new virtual session around a stream with default shield limits.
    pub fn new(stream: S) -> Self {
        Self::with_limits(stream, ShieldLimits::default())
    }

    /// Spawns a new virtual session with explicit shield limits.
    pub fn with_limits(stream: S, limits: ShieldLimits) -> Self {
        Self {
            id: SessionId::next(),
            state: SessionState::Handshake,
            username: None,
            uuid: None,
            connected_at: Instant::now(),
            last_ping: None,
            ping_ms: 0,
            shield: ShieldedStream::new(stream, limits),
            cmd_rx: None,
        }
    }

    /// Spawns a new virtual session with a command channel and explicit shield limits.
    pub fn with_command_channel(
        stream: S,
        limits: ShieldLimits,
        cmd_rx: tokio::sync::mpsc::UnboundedReceiver<SessionCommand>,
    ) -> Self {
        Self {
            id: SessionId::next(),
            state: SessionState::Handshake,
            username: None,
            uuid: None,
            connected_at: Instant::now(),
            last_ping: None,
            ping_ms: 0,
            shield: ShieldedStream::new(stream, limits),
            cmd_rx: Some(cmd_rx),
        }
    }

    /// Sends raw pre-encoded packet bytes down the shielded connection stream.
    pub async fn send_packet_bytes(&mut self, bytes: &[u8]) -> Result<(), SessionError> {
        self.shield.write_raw_bytes(bytes).await?;
        Ok(())
    }

    /// Completes handshake, login, and configuration.
    ///
    /// Returns `Ok(None)` if the connection was a Status ping request and completed.
    /// Returns `Ok(Some((username, uuid)))` if the player reached InLimbo (Play state).
    pub async fn complete_handshake_and_login(
        &mut self,
    ) -> Result<Option<(String, u128)>, SessionError> {
        self.handle_handshake().await?;

        match self.state {
            SessionState::Status => {
                self.handle_status().await?;
                Ok(None)
            }
            SessionState::Login => {
                self.handle_login().await?;
                self.handle_configuration().await?;
                let username = self
                    .username
                    .clone()
                    .unwrap_or_else(|| "Player".to_string());
                let uuid = self.uuid.unwrap_or(0);
                Ok(Some((username, uuid)))
            }
            _ => Err(SessionError::UnexpectedPacket {
                state: self.state,
                id: 0,
            }),
        }
    }

    /// Runs the session actor to completion.
    pub async fn run(&mut self) -> Result<(), SessionError> {
        if self.complete_handshake_and_login().await?.is_some() {
            self.handle_limbo_loop().await?;
        }

        self.state = SessionState::Terminated;
        Ok(())
    }

    /// Step 1: Handshake negotiation.
    async fn handle_handshake(&mut self) -> Result<(), SessionError> {
        let (packet_id, payload) = self.shield.read_packet(false).await?;
        if packet_id != 0x00 {
            return Err(SessionError::UnexpectedPacket {
                state: self.state,
                id: packet_id,
            });
        }

        let handshake = Handshake::decode(&payload)?;
        debug!(session = ?self.id, ?handshake, "Received handshake");

        match handshake.intent {
            HandshakeIntent::Status => {
                self.state = SessionState::Status;
            }
            HandshakeIntent::Login | HandshakeIntent::Transfer => {
                self.state = SessionState::Login;
            }
        }
        Ok(())
    }

    /// Step 2a: Server list ping/status response.
    async fn handle_status(&mut self) -> Result<(), SessionError> {
        let (req_id, _) = self.shield.read_packet(false).await?;
        if req_id != 0x00 {
            return Err(SessionError::UnexpectedPacket {
                state: self.state,
                id: req_id,
            });
        }

        let status_json = format!(
            r#"{{"version":{{"name":"26.3","protocol":{}}},"players":{{"max":10000,"online":1}},"description":{{"text":"§bZenith §8» §7Virtual Holding & Transit"}}}}"#,
            PROTOCOL_VERSION
        );

        let mut resp_payload = BytesMut::new();
        write_string(&status_json, &mut resp_payload);
        self.shield.write_packet(0x00, &resp_payload).await?;

        // Read ping packet and return pong
        let (ping_id, ping_payload) = self.shield.read_packet(false).await?;
        if ping_id == 0x01 {
            self.shield.write_packet(0x01, &ping_payload).await?;
        }
        Ok(())
    }

    /// Step 2b: Login negotiation.
    async fn handle_login(&mut self) -> Result<(), SessionError> {
        let (login_id, payload) = self.shield.read_packet(false).await?;
        if login_id != 0x00 {
            return Err(SessionError::UnexpectedPacket {
                state: self.state,
                id: login_id,
            });
        }

        let start = LoginStart::decode(&payload)?;
        info!(session = ?self.id, username = %start.name, "Player logging into Zenith limbo");
        self.username = Some(start.name.clone());
        self.uuid = Some(start.uuid);

        // Send Login Success (Packet ID 0x02 in Login state)
        let success = LoginSuccess {
            uuid: start.uuid,
            username: start.name,
        };
        self.shield.write_packet(0x02, &success.encode()).await?;

        // Expect Login Acknowledged (Packet ID 0x03 in Login state for 26.3)
        let (ack_id, _) = self.shield.read_packet(false).await?;
        if ack_id != 0x03 {
            return Err(SessionError::UnexpectedPacket {
                state: self.state,
                id: ack_id,
            });
        }

        self.state = SessionState::Configuration;
        Ok(())
    }

    /// Step 3: Configuration phase.
    async fn handle_configuration(&mut self) -> Result<(), SessionError> {
        // Send Finish Configuration (Packet ID 0x03 in Configuration state)
        self.shield.write_packet(0x03, &[]).await?;

        // Read client packets until client finishes configuration (Packet ID 0x03 or 0x02)
        loop {
            let (packet_id, _) = self.shield.read_packet(false).await?;
            if packet_id == 0x03 || packet_id == 0x02 {
                break;
            }
            // Ignore other configuration packets (e.g. client info 0x00, known packs 0x07)
        }

        self.state = SessionState::InLimbo;
        Ok(())
    }

    /// Step 4: Virtual holding loop in InLimbo (Play state).
    pub async fn handle_limbo_loop(&mut self) -> Result<(), SessionError> {
        // 1. Send minimal zero-chunk Login (Play) packet (Packet ID 0x32 / 50 in 26.3)
        let mut login_play = BytesMut::new();
        login_play.put_i32(self.id.0 as i32); // Entity ID
        login_play.put_u8(0); // Not hardcore
        write_varint(1, &mut login_play); // Dimension count
        write_string("minecraft:overworld", &mut login_play);
        write_varint(10000, &mut login_play); // Max players
        write_varint(2, &mut login_play); // Chunk view distance
        write_varint(2, &mut login_play); // Simulation distance
        login_play.put_u8(0); // Reduced debug info
        login_play.put_u8(1); // Show death screen
        login_play.put_u8(0); // Do limited crafting
        // Spawn info:
        write_varint(0, &mut login_play); // Dimension type ID
        write_string("minecraft:overworld", &mut login_play);
        login_play.put_i64(0); // Hashed seed
        login_play.put_u8(1); // Game mode 1 (Creative/fly)
        login_play.put_i8(-1); // Previous game mode
        login_play.put_u8(0); // Is debug
        login_play.put_u8(1); // Is flat
        login_play.put_u8(0); // Has death location
        write_varint(0, &mut login_play); // Portal cooldown
        write_varint(64, &mut login_play); // Sea level
        login_play.put_u8(0); // Enforces secure chat

        self.shield.write_packet(50, &login_play).await?;

        // 2. Send Synchronize Player Position (Packet ID 0x49 / 73 in 26.3)
        let mut pos_payload = BytesMut::new();
        write_varint(1, &mut pos_payload); // Teleport ID
        pos_payload.put_f64(0.0); // X
        pos_payload.put_f64(64.0); // Y
        pos_payload.put_f64(0.0); // Z
        pos_payload.put_f64(0.0); // Delta X
        pos_payload.put_f64(0.0); // Delta Y
        pos_payload.put_f64(0.0); // Delta Z
        pos_payload.put_f32(0.0); // Yaw
        pos_payload.put_f32(0.0); // Pitch
        pos_payload.put_i32(0); // Relative flags

        self.shield.write_packet(73, &pos_payload).await?;

        // 3. Heartbeat loop: maintain session with periodic KeepAlive and handle commands
        let mut keepalive_timer = tokio::time::interval(Duration::from_secs(10));
        let mut keepalive_counter: i64 = 1000;

        loop {
            tokio::select! {
                cmd = async {
                    match self.cmd_rx.as_mut() {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    match cmd {
                        Some(SessionCommand::SendRaw(bytes)) => {
                            self.send_packet_bytes(&bytes).await?;
                        }
                        Some(SessionCommand::Transfer { host, port }) => {
                            let mut payload = BytesMut::new();
                            write_string(&host, &mut payload);
                            write_varint(port as i32, &mut payload);
                            self.shield.write_packet(124, &payload).await?; // 0x7C Transfer
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            return Ok(());
                        }
                        Some(SessionCommand::Close) | None => {
                            return Ok(());
                        }
                    }
                }
                _ = keepalive_timer.tick() => {
                    keepalive_counter += 1;
                    let ka = KeepAlive { id: keepalive_counter };
                    self.last_ping = Some(Instant::now());
                    self.shield.write_packet(45, &ka.encode()).await?;
                }
                res = self.shield.read_packet(true) => {
                    match res {
                        Ok((id, payload)) => {
                            if id == 28 { // KeepAlive response
                                if let Ok(resp) = KeepAlive::decode(&payload) {
                                    if resp.id == keepalive_counter {
                                        if let Some(sent) = self.last_ping {
                                            self.ping_ms = sent.elapsed().as_millis() as u32;
                                        }
                                    }
                                }
                            }
                        }
                        Err(ShieldError::Wire(WireError::Io(ref msg)))
                            if msg.contains("early eof")
                                || msg.contains("channel closed")
                                || msg.contains("broken pipe")
                                || msg.contains("forcibly closed")
                                || msg.contains("reset by peer") =>
                        {
                            // Peer disconnected
                            return Ok(());
                        }
                        Err(ShieldError::SlowlorisTimeout(_)) => {
                            return Err(SessionError::ConnectionClosed);
                        }
                        Err(e) => return Err(SessionError::Shield(e)),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;
    use zenith_wire::{HandshakeIntent, read_string, write_raw_packet};

    #[tokio::test]
    async fn test_virtual_session_status_flow() {
        let (client, server) = duplex(1024);
        let mut session = VirtualSession::new(server);

        let client_task = tokio::spawn(async move {
            let mut c = client;
            // 1. Handshake (Status intent)
            let hs = Handshake {
                protocol_version: PROTOCOL_VERSION,
                server_address: "localhost".into(),
                server_port: 25565,
                intent: HandshakeIntent::Status,
            };
            write_raw_packet(&mut c, 0x00, &hs.encode()).await.unwrap();

            // 2. Status Request
            write_raw_packet(&mut c, 0x00, &[]).await.unwrap();

            // 3. Status Response
            let (id, payload) = zenith_wire::read_raw_packet(&mut c, 1024).await.unwrap();
            assert_eq!(id, 0x00);
            let s = read_string(&mut &payload[..], 1024).unwrap();
            assert!(s.contains("Zenith"));

            // 4. Ping
            write_raw_packet(&mut c, 0x01, &[1, 2, 3, 4, 5, 6, 7, 8])
                .await
                .unwrap();

            // 5. Pong
            let (pong_id, pong_payload) = zenith_wire::read_raw_packet(&mut c, 1024).await.unwrap();
            assert_eq!(pong_id, 0x01);
            assert_eq!(&pong_payload, &[1, 2, 3, 4, 5, 6, 7, 8]);
        });

        session.run().await.unwrap();
        client_task.await.unwrap();
    }
}
