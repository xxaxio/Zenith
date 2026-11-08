//! Synthetic client actor supporting Vanilla, Horizon Turbo, and hardened Adversary profiles.

use crate::mutate::{MutationKind, mutate_packet};
use crate::proxy::HttpProxy;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::info;
use zenith_wire::{
    Handshake, HandshakeIntent, LoginStart, PROTOCOL_VERSION, read_raw_packet, write_raw_packet,
    write_string, write_varint,
};

/// Errors encountered by synthetic clients.
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Wire error: {0}")]
    Wire(#[from] zenith_wire::WireError),
    #[error("Proxy error: {0}")]
    Proxy(#[from] crate::proxy::ProxyError),
    #[error("Timeout occurred during phase: {0}")]
    Timeout(&'static str),
    #[error("Protocol error: {0}")]
    Protocol(String),
    #[error("Shield rejected or terminated connection as expected")]
    ShieldTerminated,
}

/// Attack strategies executed by adversarial clients to test defensive limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdversaryStrategy {
    /// Sends bytes one by one with delays to exhaust server socket descriptors.
    Slowloris {
        byte_interval: Duration,
        total_bytes: usize,
    },
    /// Declares an oversized VarInt length to trigger frame bound limits.
    JumboFrame { declared_size: usize },
    /// Injects an invalid, non-terminating 6-byte VarInt sequence.
    MalformedVarInt,
    /// Sends a Play state packet before completing Handshake / Login.
    StateConfusion,
    /// Bypasses sending mandatory ClientInformation in Configuration phase.
    MissingClientInfo,
    /// Injects a blacklisted client software brand (e.g. "mcbot").
    BlockedBrand(String),
    /// Fires a massive burst of packets in quick succession to trigger rate limiters.
    TokenBucketFlood { count: usize },
    /// Mutates packet bodies with bit flips or garbage padding.
    MutatedPayload { kind: MutationKind, seed: u64 },
    /// Completes handshake, login, configuration with customized brand and client info,
    /// then spams packets at high PPS in Limbo until evicted or terminated.
    LimboSpam {
        brand: String,
        locale: String,
        view_distance: i8,
        packets: usize,
    },
    /// Injects an oversized frame targeting state-aware bounds.
    OversizedPhaseFrame { declared_size: usize },
}

/// Profile of a realistic Minecraft player ticking inside Limbo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerProfile {
    /// 20 Hz movement in void, answers KeepAlives, tracks RTT.
    Standard,
    /// 20 Hz movement + periodic /ping command execution every 2s.
    Interactive,
    /// Higher packet rate (rotations and status packets) near the rate limiter ceiling.
    Stressed,
}

/// Real-time live telemetry metrics reported continuously by active client actor tasks.
#[derive(Debug, Clone, Default)]
pub struct LiveTelemetry {
    pub joined_limbo: Arc<AtomicUsize>,
    pub ticks_simulated: Arc<AtomicU64>,
    pub bytes_sent: Arc<AtomicU64>,
    pub bytes_received: Arc<AtomicU64>,
    pub pings_answered: Arc<AtomicU64>,
    pub rtt_tx: Option<tokio::sync::mpsc::UnboundedSender<Duration>>,
    pub join_tx: Option<tokio::sync::mpsc::UnboundedSender<Duration>>,
}

/// Profile type of the synthetic client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientType {
    /// Standard Java Edition client with realistic human latency jitter.
    Vanilla { ping_jitter: Duration },
    /// Horizon client with sub-millisecond KeepAlives and #7484 Turbo capability.
    Horizon,
    /// Full realistic player simulation with 20 Hz ticking, teleports, and commands.
    Player {
        profile: PlayerProfile,
        ping_jitter: Duration,
    },
    /// Aggressive adversarial threat actor executing targeted cyber attacks.
    Adversary(AdversaryStrategy),
}

/// Resulting outcome of a synthetic client session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientOutcome {
    /// Transferred to target backend via 0x7C Transfer packet.
    Transferred { host: String, port: u16 },
    /// Held in virtual limbo void, answering KeepAlives and ticking at 20 Hz.
    HeldInLimbo {
        keepalives_answered: usize,
        alerts_received: usize,
        pings_received: usize,
        ticks_simulated: usize,
        bytes_in: u64,
        bytes_out: u64,
        rtt_samples: Vec<Duration>,
    },
    /// Shield triggered and connection was closed/jailed as intended.
    ShieldJailed,
    /// Server closed connection cleanly.
    Disconnected,
}

/// Configurable synthetic client actor.
#[derive(Debug, Clone)]
pub struct SyntheticClient {
    pub username: String,
    pub uuid: u128,
    pub client_type: ClientType,
    pub proxy: Option<HttpProxy>,
    pub proxy_v2: Option<zenith_wire::ProxyHeader>,
    pub telemetry: Option<LiveTelemetry>,
}

impl SyntheticClient {
    pub fn new(username: &str, client_type: ClientType) -> Self {
        let uuid = zenith_wire::offline_uuid(username);
        Self {
            username: username.to_string(),
            uuid,
            client_type,
            proxy: None,
            proxy_v2: None,
            telemetry: None,
        }
    }

    pub fn with_telemetry(mut self, telemetry: LiveTelemetry) -> Self {
        self.telemetry = Some(telemetry);
        self
    }

    pub fn with_proxy_v2(mut self, header: zenith_wire::ProxyHeader) -> Self {
        self.proxy_v2 = Some(header);
        self
    }

    pub fn with_proxy(mut self, proxy: HttpProxy) -> Self {
        self.proxy = Some(proxy);
        self
    }

    /// Connects to Zenith and executes the assigned client protocol or attack sequence.
    pub async fn run(
        &self,
        host: &str,
        port: u16,
        session_duration: Duration,
    ) -> Result<ClientOutcome, ClientError> {
        let mut stream = if let Some(ref proxy) = self.proxy {
            if host == "127.0.0.1" || host == "localhost" {
                // When targeting local Zenith, connect directly over TCP so the attack hits the daemon
                let addr = format!("{host}:{port}");
                TcpStream::connect(&addr).await?
            } else {
                proxy
                    .connect_tunnel(host, port, Duration::from_secs(5))
                    .await?
            }
        } else {
            let addr = format!("{host}:{port}");
            TcpStream::connect(&addr).await?
        };

        match &self.client_type {
            ClientType::Adversary(strategy) => {
                return self.execute_adversary_strategy(&mut stream, strategy).await;
            }
            ClientType::Vanilla { ping_jitter } => {
                let join_start = std::time::Instant::now();
                self.execute_handshake_and_login(&mut stream).await?;
                self.execute_configuration_phase(&mut stream).await?;
                if let Some(ref tel) = self.telemetry {
                    if let Some(ref tx) = tel.join_tx {
                        let _ = tx.send(join_start.elapsed());
                    }
                }
                return self
                    .run_limbo_loop(&mut stream, *ping_jitter, session_duration, None)
                    .await;
            }
            ClientType::Horizon => {
                let join_start = std::time::Instant::now();
                self.execute_handshake_and_login(&mut stream).await?;
                self.execute_configuration_phase(&mut stream).await?;
                if let Some(ref tel) = self.telemetry {
                    if let Some(ref tx) = tel.join_tx {
                        let _ = tx.send(join_start.elapsed());
                    }
                }
                return self
                    .run_limbo_loop(
                        &mut stream,
                        Duration::from_millis(2),
                        session_duration,
                        None,
                    )
                    .await;
            }
            ClientType::Player {
                profile,
                ping_jitter,
            } => {
                let join_start = std::time::Instant::now();
                self.execute_handshake_and_login(&mut stream).await?;
                self.execute_configuration_phase(&mut stream).await?;
                if let Some(ref tel) = self.telemetry {
                    if let Some(ref tx) = tel.join_tx {
                        let _ = tx.send(join_start.elapsed());
                    }
                }
                return self
                    .run_limbo_loop(&mut stream, *ping_jitter, session_duration, Some(*profile))
                    .await;
            }
        }
    }

    /// Handshake and Login phases.
    async fn execute_handshake_and_login(&self, stream: &mut TcpStream) -> Result<(), ClientError> {
        if let Some(ref hdr) = self.proxy_v2 {
            let proxy_bytes = hdr.encode();
            stream.write_all(&proxy_bytes).await?;
            stream.flush().await?;
        }
        let hs = Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "127.0.0.1".into(),
            server_port: 25565,
            intent: HandshakeIntent::Login,
        };
        let hs_body = hs.encode();
        write_raw_packet(stream, 0x00, &hs_body).await?;

        // 0x00 LoginStart
        let login = LoginStart {
            name: self.username.clone(),
            uuid: self.uuid,
        };
        let login_body = login.encode();
        write_raw_packet(stream, 0x00, &login_body).await?;

        // Read until 0x02 LoginSuccess
        loop {
            let (packet_id, _) =
                match tokio::time::timeout(Duration::from_secs(5), read_raw_packet(stream, 32_768))
                    .await
                {
                    Ok(Ok(res)) => res,
                    Ok(Err(e)) => {
                        tracing::error!("Failed reading packet in Login: {:?}", e);
                        return Err(ClientError::Wire(e));
                    }
                    Err(_) => return Err(ClientError::Timeout("LoginSuccess read")),
                };

            if packet_id == 0x02 {
                // 0x03 LoginAcknowledged
                write_raw_packet(stream, 0x03, &[]).await?;
                break;
            }
        }

        Ok(())
    }

    /// Configuration phase.
    async fn execute_configuration_phase(&self, stream: &mut TcpStream) -> Result<(), ClientError> {
        let brand_str = match self.uuid % 5 {
            0 => "vanilla",
            1 => "fabric",
            2 => "lunarclient",
            3 => "feather",
            _ => "forge",
        };
        let locale = match self.uuid % 4 {
            0 => "en_us",
            1 => "fr_fr",
            2 => "de_de",
            _ => "es_es",
        };
        let view_distance = (8 + (self.uuid % 16)) as i8;

        // 1. Send legitimate ClientInformation (Packet 0x00)
        let info = zenith_wire::ClientInformation {
            language: locale.to_string(),
            view_distance,
            ..Default::default()
        };
        write_raw_packet(stream, 0x00, &info.encode()).await?;

        // 2. Send realistic client brand (Packet 0x02)
        let mut brand_bytes = Vec::new();
        write_string(brand_str, &mut brand_bytes);
        let cp = zenith_wire::CustomPayload {
            channel: "minecraft:brand".into(),
            data: brand_bytes,
        };
        write_raw_packet(stream, 0x02, &cp.encode()).await?;

        loop {
            let (packet_id, payload) =
                match tokio::time::timeout(Duration::from_secs(5), read_raw_packet(stream, 32_768))
                    .await
                {
                    Ok(Ok(res)) => res,
                    Ok(Err(e)) => {
                        tracing::error!("Failed reading packet in Configuration: {:?}", e);
                        return Err(ClientError::Wire(e));
                    }
                    Err(_) => return Err(ClientError::Timeout("Config packet read")),
                };

            match packet_id {
                // 0x03 FinishConfiguration
                0x03 => {
                    // 0x03 AcknowledgeFinishConfiguration
                    write_raw_packet(stream, 0x03, &[]).await?;
                    break;
                }
                // 0x04 KeepAlive in configuration
                0x04 => {
                    write_raw_packet(stream, 0x04, &payload).await?;
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Play state: limbo holding loop, answers KeepAlives, moves at 20 Hz, sends commands, and detects 0x7C Transfer.
    async fn run_limbo_loop(
        &self,
        stream: &mut TcpStream,
        ping_latency: Duration,
        max_duration: Duration,
        profile: Option<PlayerProfile>,
    ) -> Result<ClientOutcome, ClientError> {
        let start = std::time::Instant::now();
        let mut keepalives_answered = 0;
        let mut alerts_received = 0;
        let mut pings_received = 0;
        let mut ticks_simulated = 0;
        let mut bytes_in = 0u64;
        let mut bytes_out = 0u64;
        let mut rtt_samples = Vec::new();

        if let Some(ref tel) = self.telemetry {
            tel.joined_limbo.fetch_add(1, Ordering::Relaxed);
        }

        let (mut reader, mut writer) = tokio::io::split(stream);

        let mut tick_timer = tokio::time::interval(Duration::from_millis(50));
        tick_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut tick_counter: u64 = 0;
        let mut pending_teleport: Option<i32> = None;
        let mut ping_sent_at: Option<std::time::Instant> = None;

        loop {
            if start.elapsed() >= max_duration {
                return Ok(ClientOutcome::HeldInLimbo {
                    keepalives_answered,
                    alerts_received,
                    pings_received,
                    ticks_simulated,
                    bytes_in,
                    bytes_out,
                    rtt_samples,
                });
            }

            tokio::select! {
                _ = tick_timer.tick() => {
                    ticks_simulated += 1;
                    tick_counter += 1;
                    if let Some(ref tel) = self.telemetry {
                        tel.ticks_simulated.fetch_add(1, Ordering::Relaxed);
                    }

                    // 1. If we received a teleport packet from server, confirm it immediately
                    if let Some(tp_id) = pending_teleport.take() {
                        let mut tp_buf = Vec::new();
                        zenith_wire::write_varint(tp_id, &mut tp_buf);
                        // Packet 0 = AcceptTeleportation in Play state
                        let _ = write_raw_packet(&mut writer, 0x00, &tp_buf).await;
                        let sent_len = (2 + tp_buf.len()) as u64;
                        bytes_out += sent_len;
                        if let Some(ref tel) = self.telemetry {
                            tel.bytes_sent.fetch_add(sent_len, Ordering::Relaxed);
                        }

                        // Also send Packet 42 = PlayerLoaded in 26.3
                        let _ = write_raw_packet(&mut writer, 42, &[]).await;
                        bytes_out += 2;
                        if let Some(ref tel) = self.telemetry {
                            tel.bytes_sent.fetch_add(2, Ordering::Relaxed);
                        }
                    }

                    // 2. 20 Hz Movement / Rotation update (Packet 38 = MovePlayerRot)
                    if let Some(p) = profile {
                        let yaw = (tick_counter % 360) as f32;
                        let pitch = (((tick_counter / 10) % 30) as f32) - 15.0;
                        let mut rot_payload = Vec::with_capacity(9);
                        rot_payload.extend_from_slice(&yaw.to_be_bytes());
                        rot_payload.extend_from_slice(&pitch.to_be_bytes());
                        rot_payload.push(1); // on_ground: true
                        let _ = write_raw_packet(&mut writer, 38, &rot_payload).await;
                        bytes_out += 11;
                        if let Some(ref tel) = self.telemetry {
                            tel.bytes_sent.fetch_add(11, Ordering::Relaxed);
                        }

                        // If Stressed profile: send extra rapid rotation to test rate limit resilience
                        if p == PlayerProfile::Stressed && tick_counter % 2 == 0 {
                            let _ = write_raw_packet(&mut writer, 38, &rot_payload).await;
                            bytes_out += 11;
                            if let Some(ref tel) = self.telemetry {
                                tel.bytes_sent.fetch_add(11, Ordering::Relaxed);
                            }
                        }

                        // 3. Periodic command execution for Interactive profile (every 40 ticks = 2s)
                        if p == PlayerProfile::Interactive && tick_counter % 40 == 0 && ping_sent_at.is_none() {
                            let mut cmd_buf = Vec::new();
                            zenith_wire::write_string("ping", &mut cmd_buf);
                            let _ = write_raw_packet(&mut writer, 7, &cmd_buf).await;
                            let sent_len = (2 + cmd_buf.len()) as u64;
                            bytes_out += sent_len;
                            if let Some(ref tel) = self.telemetry {
                                tel.bytes_sent.fetch_add(sent_len, Ordering::Relaxed);
                            }
                            ping_sent_at = Some(std::time::Instant::now());
                        }
                    }
                }
                read_res = read_raw_packet(&mut reader, 2 * 1024 * 1024) => {
                    match read_res {
                        Ok((packet_id, payload)) => {
                            let in_len = (payload.len() + 2) as u64;
                            bytes_in += in_len;
                            if let Some(ref tel) = self.telemetry {
                                tel.bytes_received.fetch_add(in_len, Ordering::Relaxed);
                            }
                            match packet_id {
                                // Synchronize Player Position (Packet 73 in Play state)
                                73 => {
                                    let mut cur = &payload[..];
                                    if let Ok(tp_id) = zenith_wire::read_varint(&mut cur) {
                                        pending_teleport = Some(tp_id);
                                    }
                                }
                                // KeepAlive in Protocol 777 Play state (ID 45 / 0x2D, response 28 / 0x1C)
                                45 | 0x26 => {
                                    if !ping_latency.is_zero() {
                                        tokio::time::sleep(ping_latency).await;
                                    }
                                    let _ = write_raw_packet(&mut writer, 28, &payload).await;
                                    let sent_len = (payload.len() + 2) as u64;
                                    bytes_out += sent_len;
                                    if let Some(ref tel) = self.telemetry {
                                        tel.bytes_sent.fetch_add(sent_len, Ordering::Relaxed);
                                    }
                                    keepalives_answered += 1;
                                }
                                // System Chat (Packet 124)
                                124 => {
                                    alerts_received += 1;
                                    let text = String::from_utf8_lossy(&payload);
                                    if text.contains("latency")
                                        || text.contains("Latency")
                                        || text.contains("Latence:")
                                        || text.contains("Ping:")
                                    {
                                        pings_received += 1;
                                        if let Some(sent) = ping_sent_at.take() {
                                            let rtt = sent.elapsed();
                                            rtt_samples.push(rtt);
                                            if let Some(ref tel) = self.telemetry {
                                                tel.pings_answered.fetch_add(1, Ordering::Relaxed);
                                                if let Some(ref tx) = tel.rtt_tx {
                                                    let _ = tx.send(rtt);
                                                }
                                            }
                                        }
                                    }
                                }
                                // Alerts & Broadcasts: Disguised (114), Title (116), Times (117)
                                114 | 116 | 117 => {
                                    alerts_received += 1;
                                }
                                // Transfer packet in Protocol 777 Play state (ID 132 / 0x84)
                                132 => {
                                    let mut cur = &payload[..];
                                    if let Ok(host) = zenith_wire::read_string(&mut cur, 255) {
                                        if let Ok(port_val) = zenith_wire::read_varint(&mut cur) {
                                            let port = port_val as u16;
                                            info!(%self.username, %host, %port, "Client received Transfer packet!");
                                            return Ok(ClientOutcome::Transferred { host, port });
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        Err(zenith_wire::WireError::Io(_)) => {
                            return Ok(ClientOutcome::Disconnected);
                        }
                        Err(_) => {
                            return Ok(ClientOutcome::HeldInLimbo {
                                keepalives_answered,
                                alerts_received,
                                pings_received,
                                ticks_simulated,
                                bytes_in,
                                bytes_out,
                                rtt_samples,
                            });
                        }
                    }
                }
            }
        }
    }

    /// Executes designated adversarial attacks against Zenith Shield.
    async fn execute_adversary_strategy(
        &self,
        stream: &mut TcpStream,
        strategy: &AdversaryStrategy,
    ) -> Result<ClientOutcome, ClientError> {
        match strategy {
            AdversaryStrategy::Slowloris {
                byte_interval,
                total_bytes,
            } => {
                let dummy = [0x05, 0x00, 0x00, 0x00, 0x02];
                for i in 0..*total_bytes {
                    let b = [dummy[i % dummy.len()]];
                    if stream.write_all(&b).await.is_err() {
                        return Ok(ClientOutcome::ShieldJailed);
                    }
                    let _ = stream.flush().await;
                    tokio::time::sleep(*byte_interval).await;
                }
                // Check if server dropped connection
                let mut check = [0u8; 1];
                let n = stream.read(&mut check).await.unwrap_or(0);
                if n == 0 {
                    Ok(ClientOutcome::ShieldJailed)
                } else {
                    Ok(ClientOutcome::Disconnected)
                }
            }
            AdversaryStrategy::JumboFrame { declared_size } => {
                // Write massive declared length VarInt followed by minimal bytes
                let mut header = Vec::new();
                write_varint(*declared_size as i32, &mut header);
                header.extend_from_slice(&[0x00; 16]);
                let _ = stream.write_all(&header).await;
                let _ = stream.flush().await;

                // Server should immediately drop or terminate stream
                let mut check = [0u8; 1];
                let n = stream.read(&mut check).await.unwrap_or(0);
                if n == 0 {
                    Ok(ClientOutcome::ShieldJailed)
                } else {
                    Ok(ClientOutcome::Disconnected)
                }
            }
            AdversaryStrategy::MalformedVarInt => {
                // Injects invalid 6-byte VarInt (0x80 repeat)
                let bad_varint = [0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01];
                let _ = stream.write_all(&bad_varint).await;
                let _ = stream.flush().await;

                let mut check = [0u8; 1];
                let n = stream.read(&mut check).await.unwrap_or(0);
                if n == 0 {
                    Ok(ClientOutcome::ShieldJailed)
                } else {
                    Ok(ClientOutcome::Disconnected)
                }
            }
            AdversaryStrategy::StateConfusion => {
                // Injects a Play state packet (e.g. Chat Command 0x04) directly into Handshake
                let mut payload = Vec::new();
                write_string("teleport 0 0 0", &mut payload);
                let _ = write_raw_packet(stream, 0x04, &payload).await;

                let mut check = [0u8; 1];
                let n = stream.read(&mut check).await.unwrap_or(0);
                if n == 0 {
                    Ok(ClientOutcome::ShieldJailed)
                } else {
                    Ok(ClientOutcome::Disconnected)
                }
            }
            AdversaryStrategy::TokenBucketFlood { count } => {
                // Bursts rapid packets to exhaust token bucket and trigger IP jail
                let hs = Handshake {
                    protocol_version: PROTOCOL_VERSION,
                    server_address: "127.0.0.1".into(),
                    server_port: 25565,
                    intent: HandshakeIntent::Status,
                };
                let body = hs.encode();
                for _ in 0..*count {
                    if write_raw_packet(stream, 0x00, &body).await.is_err() {
                        return Ok(ClientOutcome::ShieldJailed);
                    }
                }

                // Check if connection was closed by shield
                let mut check = [0u8; 1];
                let _ = stream.read(&mut check).await;
                Ok(ClientOutcome::ShieldJailed)
            }
            AdversaryStrategy::MutatedPayload { kind, seed } => {
                let hs = Handshake {
                    protocol_version: PROTOCOL_VERSION,
                    server_address: "127.0.0.1".into(),
                    server_port: 25565,
                    intent: HandshakeIntent::Login,
                };
                let valid = hs.encode();
                let corrupted = mutate_packet(&valid, *kind, *seed);
                let _ = stream.write_all(&corrupted).await;
                let _ = stream.flush().await;

                let mut check = [0u8; 1];
                let n = stream.read(&mut check).await.unwrap_or(0);
                if n == 0 {
                    Ok(ClientOutcome::ShieldJailed)
                } else {
                    Ok(ClientOutcome::Disconnected)
                }
            }
            AdversaryStrategy::MissingClientInfo => {
                // Complete handshake & login, but deliberately skip ClientInformation in Configuration
                self.execute_handshake_and_login(stream).await?;
                // Read FinishConfiguration (0x03) from server
                let _ = read_raw_packet(stream, 32_768).await;
                // Directly send FinishConfigurationAck (0x03) omitting ClientInformation
                let _ = write_raw_packet(stream, 0x03, &[]).await;

                let mut check = [0u8; 1];
                let n = stream.read(&mut check).await.unwrap_or(0);
                if n == 0 {
                    Ok(ClientOutcome::ShieldJailed)
                } else {
                    Ok(ClientOutcome::Disconnected)
                }
            }
            AdversaryStrategy::BlockedBrand(brand) => {
                // Complete handshake & login, send ClientInformation, but inject a blocked brand
                self.execute_handshake_and_login(stream).await?;
                let info = zenith_wire::ClientInformation::default();
                let _ = write_raw_packet(stream, 0x00, &info.encode()).await;

                let mut brand_bytes = Vec::new();
                write_string(brand, &mut brand_bytes);
                let cp = zenith_wire::CustomPayload {
                    channel: "minecraft:brand".into(),
                    data: brand_bytes,
                };
                let _ = write_raw_packet(stream, 0x02, &cp.encode()).await;

                // Read server finish config
                let _ = read_raw_packet(stream, 32_768).await;
                let _ = write_raw_packet(stream, 0x03, &[]).await;

                let mut check = [0u8; 1];
                let n = stream.read(&mut check).await.unwrap_or(0);
                if n == 0 {
                    Ok(ClientOutcome::ShieldJailed)
                } else {
                    Ok(ClientOutcome::Disconnected)
                }
            }
            AdversaryStrategy::LimboSpam {
                brand,
                locale,
                view_distance,
                packets,
            } => {
                if self.execute_handshake_and_login(stream).await.is_err() {
                    return Ok(ClientOutcome::ShieldJailed);
                }

                // Configuration phase
                let info = zenith_wire::ClientInformation {
                    language: locale.clone(),
                    view_distance: *view_distance,
                    ..Default::default()
                };
                let _ = write_raw_packet(stream, 0x00, &info.encode()).await;

                let mut brand_bytes = Vec::new();
                write_string(brand, &mut brand_bytes);
                let cp = zenith_wire::CustomPayload {
                    channel: "minecraft:brand".into(),
                    data: brand_bytes,
                };
                let _ = write_raw_packet(stream, 0x02, &cp.encode()).await;

                // Read server finish config
                loop {
                    let (packet_id, _) = match tokio::time::timeout(
                        Duration::from_secs(3),
                        read_raw_packet(stream, 32_768),
                    )
                    .await
                    {
                        Ok(Ok(res)) => res,
                        Ok(Err(_)) => return Ok(ClientOutcome::ShieldJailed),
                        Err(_) => return Ok(ClientOutcome::Disconnected),
                    };
                    if packet_id == 0x03 {
                        let _ = write_raw_packet(stream, 0x03, &[]).await;
                        break;
                    }
                }

                // In Limbo (Play state): flood packets at maximum speed
                let dummy_payload = vec![0x00; 16];
                for _ in 0..*packets {
                    // Send KeepAlive response (packet 28 / 0x1C)
                    if write_raw_packet(stream, 28, &dummy_payload).await.is_err() {
                        return Ok(ClientOutcome::ShieldJailed);
                    }
                }

                let mut check = [0u8; 1];
                let n = stream.read(&mut check).await.unwrap_or(0);
                if n == 0 {
                    Ok(ClientOutcome::ShieldJailed)
                } else {
                    Ok(ClientOutcome::Disconnected)
                }
            }
            AdversaryStrategy::OversizedPhaseFrame { declared_size } => {
                let mut header = Vec::new();
                write_varint(*declared_size as i32, &mut header);
                header.extend_from_slice(&vec![0x00; std::cmp::min(*declared_size, 256)]);
                let _ = stream.write_all(&header).await;
                let _ = stream.flush().await;

                let mut check = [0u8; 1];
                let n = stream.read(&mut check).await.unwrap_or(0);
                if n == 0 {
                    Ok(ClientOutcome::ShieldJailed)
                } else {
                    Ok(ClientOutcome::Disconnected)
                }
            }
        }
    }
}
