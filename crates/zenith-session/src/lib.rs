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

pub mod commands;

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
    /// Dispatches the Protocol 777 Transfer packet (ID 132 in Play state) and closes the session.
    Transfer { host: String, port: u16 },
    /// Dispatches a resource pack preloading push (Packet ID 82 in Play state).
    PushResourcePack {
        uuid: u128,
        url: String,
        hash: String,
        required: bool,
        prompt: Option<String>,
    },
    /// Dispatches a broadcast alert across the screen (Title + Subtitle) and system chat.
    BroadcastAlert {
        title: String,
        subtitle: Option<String>,
        chat: String,
    },
    /// Terminates the session cleanly.
    Close,
}

/// Events sent by a session actor up to the server coordinator.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// Player executed a chat command.
    Command {
        session_id: SessionId,
        username: String,
        command: String,
        args: Vec<String>,
    },
    /// Measured round-trip ping updated via KeepAlive.
    PingUpdated { session_id: SessionId, ping_ms: u32 },
    /// Resource pack status update from client (Packet ID 44 in Play state).
    ResourcePackStatus {
        session_id: SessionId,
        uuid: u128,
        action: zenith_wire::ResourcePackAction,
    },
    /// Session ended / disconnected.
    Disconnected { session_id: SessionId },
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
    pub target_server: String,
    shield: ShieldedStream<S>,
    pub cmd_rx: Option<tokio::sync::mpsc::UnboundedReceiver<SessionCommand>>,
    pub event_tx: Option<tokio::sync::mpsc::UnboundedSender<SessionEvent>>,
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
            target_server: "127.0.0.1:25566".to_string(),
            shield: ShieldedStream::new(stream, limits),
            cmd_rx: None,
            event_tx: None,
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
            target_server: "127.0.0.1:25566".to_string(),
            shield: ShieldedStream::new(stream, limits),
            cmd_rx: Some(cmd_rx),
            event_tx: None,
        }
    }

    /// Configures the backend target server string for manual/automatic transfers.
    pub fn set_target_server(&mut self, target: String) {
        self.target_server = target;
    }

    /// Configures the event sender channel for notifying the server coordinator of commands and disconnects.
    pub fn set_event_sender(&mut self, tx: tokio::sync::mpsc::UnboundedSender<SessionEvent>) {
        self.event_tx = Some(tx);
    }

    /// Sends a system chat message (packet ID 124 in Protocol 777) into the player's chat box.
    pub async fn send_system_chat(&mut self, text: &str) -> Result<(), SessionError> {
        let mut payload = BytesMut::with_capacity(text.len() + 16);
        zenith_wire::write_nbt_string(text, &mut payload);
        payload.put_u8(0); // overlay: false (chat box)
        self.shield.write_packet(124, &payload).await?; // 124 = SystemChat in 26.3
        Ok(())
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
        // Bound loop iterations to prevent connection pinning via indefinite packet streaming.
        let mut packets_read = 0;
        loop {
            packets_read += 1;
            if packets_read > 16 {
                return Err(SessionError::UnexpectedPacket {
                    state: self.state,
                    id: 0xFF,
                });
            }
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
        write_string("minecraft:the_end", &mut login_play);
        write_varint(10000, &mut login_play); // Max players
        write_varint(2, &mut login_play); // Chunk view distance
        write_varint(2, &mut login_play); // Simulation distance
        login_play.put_u8(0); // Reduced debug info
        login_play.put_u8(1); // Show death screen
        login_play.put_u8(0); // Do limited crafting
        // Spawn info:
        write_varint(0, &mut login_play); // Dimension type ID
        write_string("minecraft:the_end", &mut login_play); // Dimension
        login_play.put_i64(0); // Hashed seed
        write_varint(3, &mut login_play); // Game mode (VarInt: 3 = Spectator for zero-chunk instant entry)
        write_varint(0, &mut login_play); // Previous game mode (OptVarInt: 0 = None)
        login_play.put_u8(0); // Is debug (Bool)
        login_play.put_u8(1); // Is flat (Bool)
        login_play.put_u8(0); // Has death location (Opt<GlobalPos>: 0 = None)
        write_varint(0, &mut login_play); // Portal cooldown (VarInt)
        write_varint(64, &mut login_play); // Sea level (VarInt)

        login_play.put_u8(0); // online_mode (Bool)
        login_play.put_u8(0); // Enforces secure chat (Bool)

        self.shield.write_packet(50, &login_play).await?;

        // 2. Send GameEvent: LEVEL_CHUNKS_LOAD_START (Packet ID 39, Event 13)
        // Signals client to exit WaitingForServer and dismiss the "Loading terrain..." screen
        let mut ge = BytesMut::new();
        ge.put_u8(13); // event: LEVEL_CHUNKS_LOAD_START
        ge.put_f32(0.0); // param: 0.0
        self.shield.write_packet(39, &ge).await?;

        // 3. Send ChunkBatchFinished (Packet ID 11, Batch Size 0)
        let mut cbf = BytesMut::new();
        write_varint(0, &mut cbf);
        self.shield.write_packet(11, &cbf).await?;

        // 4. Send Synchronize Player Position (Packet ID 0x49 / 73 in 26.3)
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

        // 5. Send Commands tree (Packet ID 0x10 / 16 in 26.3) for client-side Brigadier autocompletion
        let commands_payload = commands::build_commands_payload();
        self.shield.write_packet(16, &commands_payload).await?;

        // 3. Clean, concise 1-line welcome in chat
        let user = self.username.clone().unwrap_or_else(|| "Player".into());
        let _ = self
            .send_system_chat(&format!(
                "§b§lᴢᴇɴɪᴛʜ §8» §7Welcome §f{}§7. Please hold while waiting in queue.",
                user
            ))
            .await;

        // 4. Heartbeat loop: maintain session with periodic KeepAlive and handle commands
        let mut keepalive_timer = tokio::time::interval(Duration::from_secs(5));
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
                            self.shield.write_packet(132, &payload).await?; // 132 = Transfer in Protocol 777 Play state
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            if let Some(ref tx) = self.event_tx {
                                let _ = tx.send(SessionEvent::Disconnected { session_id: self.id });
                            }
                            return Ok(());
                        }
                        Some(SessionCommand::PushResourcePack { uuid, url, hash, required, prompt }) => {
                            let push = zenith_wire::ResourcePackPush {
                                uuid,
                                url,
                                hash,
                                required,
                                prompt,
                            };
                            self.shield.write_packet(82, &push.encode()).await?; // 82 = ResourcePackPush in 26.3 Play state
                        }
                        Some(SessionCommand::BroadcastAlert { title, subtitle, chat }) => {
                            // 1. Title animation times (Packet ID 117): 10 ticks fade in, 70 ticks stay, 20 ticks fade out
                            let mut times_payload = BytesMut::with_capacity(12);
                            times_payload.put_i32(10);
                            times_payload.put_i32(70);
                            times_payload.put_i32(20);
                            self.shield.write_packet(117, &times_payload).await?;

                            // 2. Title (Packet ID 116)
                            let mut title_payload = BytesMut::with_capacity(title.len() + 8);
                            zenith_wire::write_nbt_string(&title, &mut title_payload);
                            self.shield.write_packet(116, &title_payload).await?;

                            // 3. Optional subtitle (Packet ID 114)
                            if let Some(sub) = subtitle {
                                let mut sub_payload = BytesMut::with_capacity(sub.len() + 8);
                                zenith_wire::write_nbt_string(&sub, &mut sub_payload);
                                self.shield.write_packet(114, &sub_payload).await?;
                            }

                            // 4. System chat message (Packet ID 124)
                            if !chat.is_empty() {
                                let _ = self.send_system_chat(&chat).await;
                            }
                        }
                        Some(SessionCommand::Close) | None => {
                            if let Some(ref tx) = self.event_tx {
                                let _ = tx.send(SessionEvent::Disconnected { session_id: self.id });
                            }
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
                                            let measured = sent.elapsed().as_millis() as u32;
                                            self.ping_ms = measured;
                                            if let Some(ref tx) = self.event_tx {
                                                let _ = tx.send(SessionEvent::PingUpdated {
                                                    session_id: self.id,
                                                    ping_ms: measured,
                                                });
                                            }
                                        }
                                    }
                                }
                            } else if id == 44 { // ResourcePackResponse in Play state
                                if let Ok(resp) = zenith_wire::ResourcePackResponse::decode(&payload) {
                                    info!(session = ?self.id, ?resp.action, "Player resource pack response received");
                                    if let Some(ref tx) = self.event_tx {
                                        let _ = tx.send(SessionEvent::ResourcePackStatus {
                                            session_id: self.id,
                                            uuid: resp.uuid,
                                            action: resp.action,
                                        });
                                    }
                                }
                            } else if id == 7 || id == 8 || id == 9 {
                                // 7 = ChatCommand, 8 = ChatCommandSigned, 9 = Chat in Protocol 777
                                let raw_text = match id {
                                    7 | 8 => zenith_wire::read_string(&mut &payload[..], 256).ok(),
                                    9 => {
                                        // Zenith is a zero-chunk transit holding gatekeeper: regular player chat is disabled.
                                        // Only commands starting with '/' are accepted.
                                        if let Ok(msg) = zenith_wire::read_string(&mut &payload[..], 256) {
                                            let msg = msg.trim();
                                            msg.strip_prefix('/').map(|cmd| cmd.to_string())
                                        } else {
                                            None
                                        }
                                    }
                                    _ => None,
                                };

                                if let Some(cmd_line) = raw_text {
                                    let parts: Vec<&str> = cmd_line.split_whitespace().collect();
                                    let cmd = parts.first().copied().unwrap_or("").to_lowercase();
                                    let args: Vec<String> = parts.iter().skip(1).map(|s| s.to_string()).collect();

                                    if let Some(ref tx) = self.event_tx {
                                        let user = self.username.clone().unwrap_or_else(|| "Player".to_string());
                                        let _ = tx.send(SessionEvent::Command {
                                            session_id: self.id,
                                            username: user,
                                            command: cmd,
                                            args,
                                        });
                                    } else {
                                        // Standalone fallback (e.g. tests)
                                        match cmd.as_str() {
                                            "transfer" | "join" | "play" => {
                                                let target = self.target_server.clone();
                                                let (host, port) = match target.split_once(':') {
                                                    Some((h, p)) => (h.to_string(), p.parse::<u16>().unwrap_or(25565)),
                                                    None => (target.clone(), 25565),
                                                };
                                                let mut payload = BytesMut::new();
                                                write_string(&host, &mut payload);
                                                write_varint(port as i32, &mut payload);
                                                self.shield.write_packet(132, &payload).await?;
                                                tokio::time::sleep(Duration::from_millis(50)).await;
                                                return Ok(());
                                            }
                                            "ping" => {
                                                let _ = self.send_system_chat(&format!("§b§lᴢᴇɴɪᴛʜ §8» §7Latence: §a{}ms", self.ping_ms)).await;
                                            }
                                            _ => {}
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
                            if let Some(ref tx) = self.event_tx {
                                let _ = tx.send(SessionEvent::Disconnected { session_id: self.id });
                            }
                            return Ok(());
                        }
                        Err(ShieldError::SlowlorisTimeout(_)) => {
                            if let Some(ref tx) = self.event_tx {
                                let _ = tx.send(SessionEvent::Disconnected { session_id: self.id });
                            }
                            return Err(SessionError::ConnectionClosed);
                        }
                        Err(e) => {
                            if let Some(ref tx) = self.event_tx {
                                let _ = tx.send(SessionEvent::Disconnected { session_id: self.id });
                            }
                            return Err(SessionError::Shield(e));
                        }
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
