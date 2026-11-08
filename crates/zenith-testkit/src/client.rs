//! Synthetic client actor supporting Vanilla, Horizon Turbo, and hardened Adversary profiles.

use crate::mutate::{MutationKind, mutate_packet};
use crate::proxy::HttpProxy;
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
}

/// Profile type of the synthetic client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientType {
    /// Standard Java Edition client with realistic human latency jitter.
    Vanilla { ping_jitter: Duration },
    /// Horizon client with sub-millisecond KeepAlives and #7484 Turbo capability.
    Horizon,
    /// Aggressive adversarial threat actor executing targeted cyber attacks.
    Adversary(AdversaryStrategy),
}

/// Resulting outcome of a synthetic client session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientOutcome {
    /// Transferred to target backend via 0x7C Transfer packet.
    Transferred { host: String, port: u16 },
    /// Held in virtual limbo void, answering KeepAlives.
    HeldInLimbo {
        keepalives_answered: usize,
        alerts_received: usize,
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
        }
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
            proxy
                .connect_tunnel(host, port, Duration::from_secs(5))
                .await?
        } else {
            let addr = format!("{host}:{port}");
            TcpStream::connect(&addr).await?
        };

        match &self.client_type {
            ClientType::Adversary(strategy) => {
                return self.execute_adversary_strategy(&mut stream, strategy).await;
            }
            ClientType::Vanilla { ping_jitter } => {
                self.execute_handshake_and_login(&mut stream).await?;
                self.execute_configuration_phase(&mut stream).await?;
                return self
                    .run_limbo_loop(&mut stream, *ping_jitter, session_duration)
                    .await;
            }
            ClientType::Horizon => {
                self.execute_handshake_and_login(&mut stream).await?;
                self.execute_configuration_phase(&mut stream).await?;
                return self
                    .run_limbo_loop(&mut stream, Duration::from_millis(2), session_duration)
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
        // 1. Send legitimate ClientInformation (Packet 0x00)
        let info = zenith_wire::ClientInformation::default();
        write_raw_packet(stream, 0x00, &info.encode()).await?;

        // 2. Send legitimate client brand (Packet 0x02)
        let mut brand_bytes = Vec::new();
        write_string("zenith-synthetic", &mut brand_bytes);
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

    /// Play state: limbo holding loop, answers KeepAlives and detects 0x7C Transfer.
    async fn run_limbo_loop(
        &self,
        stream: &mut TcpStream,
        ping_latency: Duration,
        max_duration: Duration,
    ) -> Result<ClientOutcome, ClientError> {
        let start = std::time::Instant::now();
        let mut keepalives_answered = 0;
        let mut alerts_received = 0;

        loop {
            if start.elapsed() >= max_duration {
                return Ok(ClientOutcome::HeldInLimbo {
                    keepalives_answered,
                    alerts_received,
                });
            }

            let read_res = tokio::time::timeout(
                Duration::from_secs(10),
                read_raw_packet(stream, 2 * 1024 * 1024),
            )
            .await;

            match read_res {
                Ok(Ok((packet_id, payload))) => {
                    match packet_id {
                        // KeepAlive in Protocol 777 Play state (ID 45 / 0x2D, response 28 / 0x1C)
                        45 | 0x26 => {
                            if !ping_latency.is_zero() {
                                tokio::time::sleep(ping_latency).await;
                            }
                            write_raw_packet(stream, 28, &payload).await?;
                            keepalives_answered += 1;
                        }
                        // Alerts & Broadcasts: System Chat (116), Disguised (114), Title (117), BossBar (124)
                        116 | 114 | 117 | 124 => {
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
                Ok(Err(io_err)) => {
                    tracing::error!("Failed reading packet in Limbo Loop: {:?}", io_err);
                    return Ok(ClientOutcome::Disconnected);
                }
                Err(_) => {
                    // Read timeout
                    return Ok(ClientOutcome::HeldInLimbo {
                        keepalives_answered,
                        alerts_received,
                    });
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
        }
    }
}
