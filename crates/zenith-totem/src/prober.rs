//! Low-level Minecraft Server List Ping (SLP) network prober.

use std::time::{Duration, Instant};

use bytes::BytesMut;
use serde::Deserialize;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tracing::{debug, warn};

use zenith_wire::{
    PROTOCOL_VERSION, encode_packet, read_raw_packet, read_string, write_raw_packet, write_string,
    write_varint,
};

use crate::health::ProbeReport;

/// Partial JSON structure deserialized from Minecraft Status Response.
#[derive(Debug, Deserialize)]
struct SlpResponseJson {
    version: Option<SlpVersionJson>,
    players: Option<SlpPlayersJson>,
    description: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct SlpVersionJson {
    name: Option<String>,
    protocol: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct SlpPlayersJson {
    max: Option<u32>,
    online: Option<u32>,
}

/// Active SLP (Server List Ping) network prober for backend game servers.
#[derive(Debug, Clone)]
pub struct BackendProber {
    /// Target network address in `host:port` format.
    pub target_addr: String,
    /// Hostname passed in Handshake packet (defaults to 127.0.0.1 or parsed host).
    pub handshake_host: String,
    /// Port passed in Handshake packet.
    pub handshake_port: u16,
    /// Timeout for connection and handshake response.
    pub timeout_dur: Duration,
}

impl BackendProber {
    /// Creates a new prober targeting the specified address.
    #[must_use]
    pub fn new(target_addr: impl Into<String>, timeout_dur: Duration) -> Self {
        let addr_str = target_addr.into();
        let (host, port) = parse_host_port(&addr_str);
        Self {
            target_addr: addr_str,
            handshake_host: host,
            handshake_port: port,
            timeout_dur,
        }
    }

    /// Performs an active SLP probe against the target server.
    pub async fn probe(&self) -> ProbeReport {
        let start = Instant::now();
        let res = timeout(self.timeout_dur, self.probe_internal()).await;

        match res {
            Ok(Ok((report, rtt))) => {
                debug!(
                    target: "zenith_totem",
                    target_addr = %self.target_addr,
                    latency_ms = rtt.as_millis(),
                    online_players = ?report.online_players,
                    "SLP probe succeeded"
                );
                report
            }
            Ok(Err(err)) => {
                debug!(
                    target: "zenith_totem",
                    target_addr = %self.target_addr,
                    elapsed_ms = start.elapsed().as_millis(),
                    error = %err,
                    "SLP probe connection failed"
                );
                ProbeReport::failure(err)
            }
            Err(_) => {
                warn!(
                    target: "zenith_totem",
                    target_addr = %self.target_addr,
                    timeout_ms = self.timeout_dur.as_millis(),
                    "SLP probe timed out"
                );
                ProbeReport::failure("Probe connection timed out")
            }
        }
    }

    async fn probe_internal(&self) -> Result<(ProbeReport, Duration), String> {
        let connect_start = Instant::now();
        let mut stream = TcpStream::connect(&self.target_addr)
            .await
            .map_err(|e| format!("TCP connect failed: {e}"))?;

        // 1. Build Handshake packet:
        // [Packet ID: 0x00] [Protocol: VarInt] [Host: String] [Port: u16] [NextState: VarInt (1 = Status)]
        let mut handshake_payload = BytesMut::new();
        write_varint(PROTOCOL_VERSION, &mut handshake_payload);
        write_string(&self.handshake_host, &mut handshake_payload);
        handshake_payload.extend_from_slice(&self.handshake_port.to_be_bytes());
        write_varint(1, &mut handshake_payload); // 1 = Status

        let handshake_bytes = encode_packet(0x00, &handshake_payload);
        tokio::io::AsyncWriteExt::write_all(&mut stream, &handshake_bytes)
            .await
            .map_err(|e| format!("Failed writing Handshake packet: {e}"))?;

        // 2. Send Status Request packet (Packet ID: 0x00, Empty payload)
        write_raw_packet(&mut stream, 0x00, &[])
            .await
            .map_err(|e| format!("Failed writing Status Request packet: {e}"))?;

        // 3. Read Status Response packet (Packet ID: 0x00, String: JSON)
        let (packet_id, payload) = read_raw_packet(&mut stream, 1024 * 1024)
            .await
            .map_err(|e| format!("Failed reading Status Response packet: {e}"))?;

        let rtt = connect_start.elapsed();
        let latency_ms = rtt.as_millis().min(u32::MAX as u128) as u32;

        if packet_id != 0x00 {
            return Err(format!("Unexpected response packet ID: {packet_id}"));
        }

        let mut slice = &payload[..];
        let json_str = read_string(&mut slice, 1024 * 1024)
            .map_err(|e| format!("Invalid SLP JSON string: {e}"))?;

        // Parse JSON response
        let (protocol, version, online, max, motd) =
            match serde_json::from_str::<SlpResponseJson>(&json_str) {
                Ok(parsed) => {
                    let p_ver = parsed.version.as_ref().and_then(|v| v.protocol);
                    let v_name = parsed.version.and_then(|v| v.name);
                    let on = parsed.players.as_ref().and_then(|p| p.online);
                    let mx = parsed.players.and_then(|p| p.max);
                    let desc = parsed.description.map(|d| match d {
                        serde_json::Value::String(s) => s,
                        serde_json::Value::Object(map) => map
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        other => other.to_string(),
                    });
                    (p_ver, v_name, on, mx, desc)
                }
                Err(e) => {
                    debug!("Failed parsing SLP JSON (using raw): {e}");
                    (None, None, None, None, Some(json_str))
                }
            };

        Ok((
            ProbeReport::success(latency_ms, protocol, version, online, max, motd),
            rtt,
        ))
    }
}

fn parse_host_port(addr: &str) -> (String, u16) {
    if let Some((h, p)) = addr.split_once(':') {
        let port = p.parse::<u16>().unwrap_or(25565);
        (h.to_string(), port)
    } else {
        (addr.to_string(), 25565)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use zenith_wire::write_string;

    #[tokio::test]
    async fn test_slp_probe_mock_server() {
        // Start mock SLP listener
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let addr = format!("127.0.0.1:{port}");

        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            // Read Handshake
            let (hs_id, _hs_payload) = read_raw_packet(&mut socket, 1024).await.unwrap();
            assert_eq!(hs_id, 0x00);

            // Read Status Request
            let (req_id, _) = read_raw_packet(&mut socket, 1024).await.unwrap();
            assert_eq!(req_id, 0x00);

            // Send Status Response
            let json = r#"{"version":{"name":"26.3","protocol":777},"players":{"max":100,"online":42},"description":{"text":"Zenith Mock Server"}}"#;
            let mut payload = BytesMut::new();
            write_string(json, &mut payload);

            write_raw_packet(&mut socket, 0x00, &payload).await.unwrap();
        });

        let prober = BackendProber::new(addr, Duration::from_millis(1000));
        let report = prober.probe().await;

        assert!(report.is_online);
        assert_eq!(report.protocol_version, Some(777));
        assert_eq!(report.version_name.as_deref(), Some("26.3"));
        assert_eq!(report.online_players, Some(42));
        assert_eq!(report.max_players, Some(100));
        assert_eq!(report.motd.as_deref(), Some("Zenith Mock Server"));
    }

    #[tokio::test]
    async fn test_slp_probe_unreachable_server() {
        let prober = BackendProber::new("127.0.0.1:59999", Duration::from_millis(150));
        let report = prober.probe().await;
        assert!(!report.is_online);
        assert!(report.error.is_some());
    }
}
