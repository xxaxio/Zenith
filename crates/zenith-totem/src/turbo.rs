//! Horizon Native Turbo Fast-Path: Sub-millisecond IPC health telemetry
//! and pre-warmed player arrival hand-overs (`#7484` protocol).

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

#[cfg(unix)]
use tokio::net::UnixStream;

/// Errors that can occur during Horizon Turbo IPC communications.
#[derive(Debug, Error)]
pub enum TurboError {
    #[error("IO error connecting to Horizon control endpoint: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization / JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("IPC communication timed out ({0:?})")]
    Timeout(Duration),
    #[error("Horizon engine rejected command: {0}")]
    Rejected(String),
    #[error("Control endpoint path not configured or unsupported on this platform")]
    UnsupportedEndpoint,
    #[error("Invalid character or delimiter in IPC command parameter: {0}")]
    InvalidParameter(String),
}

/// Control endpoint configuration for Horizon Turbo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "address")]
pub enum TurboEndpoint {
    /// Localhost TCP port for Horizon control socket.
    Tcp(SocketAddr),
    /// Unix domain socket path (e.g. `/run/horizon/ctl.sock` or `.horizon/ctl.sock`).
    Unix(PathBuf),
}

impl TurboEndpoint {
    /// Parses an endpoint from a string (e.g. `127.0.0.1:40000` or `/path/to/ctl.sock`).
    pub fn parse_str(s: &str) -> Self {
        if let Ok(addr) = s.parse::<SocketAddr>() {
            Self::Tcp(addr)
        } else {
            Self::Unix(PathBuf::from(s))
        }
    }
}

/// Instantaneous microsecond-precision health status returned by Horizon engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HorizonTurboStatus {
    /// Microsecond tick execution duration (MSPT in µs).
    pub mspt_us: u64,
    /// Current ticks per second (nominal 20.0).
    pub tps: f64,
    /// Number of online players currently in Horizon world.
    pub online_players: usize,
    /// Maximum player capacity configured on Horizon backend.
    pub max_players: usize,
    /// Active dimension count.
    #[serde(default)]
    pub dimensions: usize,
}

/// Client for Horizon Turbo Mode IPC fast-path.
#[derive(Debug, Clone)]
pub struct HorizonTurboClient {
    endpoint: TurboEndpoint,
    timeout: Duration,
}

impl HorizonTurboClient {
    pub fn new(endpoint: TurboEndpoint) -> Self {
        Self {
            endpoint,
            timeout: Duration::from_millis(500),
        }
    }

    pub fn with_timeout(endpoint: TurboEndpoint, timeout: Duration) -> Self {
        Self { endpoint, timeout }
    }

    /// Queries the sub-millisecond status of Horizon dedicated server.
    pub async fn query_status(&self) -> Result<HorizonTurboStatus, TurboError> {
        let resp = self.send_line("status\n").await?;
        let status: HorizonTurboStatus = serde_json::from_str(&resp)?;
        Ok(status)
    }

    /// Pre-warms player arrival in Horizon RAM ahead of packet arrival (`#7484` protocol).
    ///
    /// This instructs Horizon to pre-load spawn chunks, allocate player entities
    /// and prepare memory, guaranteeing a zero-freeze transfer.
    pub async fn notify_player_expect(
        &self,
        username: &str,
        uuid: u128,
        target_world: Option<&str>,
    ) -> Result<(), TurboError> {
        if username.contains(['\r', '\n', '\0', ' ']) {
            return Err(TurboError::InvalidParameter(
                "username contains illegal characters".into(),
            ));
        }
        let world = target_world.unwrap_or("overworld");
        if world.contains(['\r', '\n', '\0', ' ']) {
            return Err(TurboError::InvalidParameter(
                "target_world contains illegal characters".into(),
            ));
        }
        let line = format!("player-expect {username} {uuid:032x} {world}\n");
        let resp = self.send_line(&line).await?;

        if resp.contains("error") {
            return Err(TurboError::Rejected(resp));
        }
        Ok(())
    }

    /// Confirms that player transfer packet was dispatched down the wire.
    pub async fn notify_player_confirm(&self, username: &str) -> Result<(), TurboError> {
        if username.contains(['\r', '\n', '\0', ' ']) {
            return Err(TurboError::InvalidParameter(
                "username contains illegal characters".into(),
            ));
        }
        let line = format!("player-confirm {username}\n");
        let resp = self.send_line(&line).await?;

        if resp.contains("error") {
            return Err(TurboError::Rejected(resp));
        }
        Ok(())
    }

    /// Sends a line down the IPC endpoint and reads the single JSON line response.
    async fn send_line(&self, line: &str) -> Result<String, TurboError> {
        if line.trim_end_matches(['\r', '\n']).contains(['\r', '\n']) {
            return Err(TurboError::InvalidParameter(
                "command line contains embedded newlines".into(),
            ));
        }
        tokio::time::timeout(self.timeout, async {
            match &self.endpoint {
                TurboEndpoint::Tcp(addr) => {
                    let mut stream = TcpStream::connect(addr).await?;
                    stream.write_all(line.as_bytes()).await?;
                    stream.flush().await?;

                    let mut reader = BufReader::new(stream);
                    let mut resp = String::new();
                    reader.read_line(&mut resp).await?;
                    Ok(resp.trim().to_string())
                }
                TurboEndpoint::Unix(path) => {
                    #[cfg(unix)]
                    {
                        let mut stream = UnixStream::connect(path).await?;
                        stream.write_all(line.as_bytes()).await?;
                        stream.flush().await?;

                        let mut reader = BufReader::new(stream);
                        let mut resp = String::new();
                        reader.read_line(&mut resp).await?;
                        Ok(resp.trim().to_string())
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = path;
                        Err(TurboError::UnsupportedEndpoint)
                    }
                }
            }
        })
        .await
        .map_err(|_| TurboError::Timeout(self.timeout))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn test_turbo_tcp_status_query() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Spawn mock Horizon control listener
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut reader = BufReader::new(&mut socket);
                let mut line = String::new();
                let _ = reader.read_line(&mut line).await;

                if line.trim() == "status" {
                    let json = r#"{"mspt_us": 4200, "tps": 20.0, "online_players": 12, "max_players": 200, "dimensions": 3}"#;
                    let _ = socket.write_all(format!("{json}\n").as_bytes()).await;
                }
            }
        });

        let client = HorizonTurboClient::new(TurboEndpoint::Tcp(addr));
        let status = client.query_status().await.unwrap();

        assert_eq!(status.mspt_us, 4200);
        assert!((status.tps - 20.0).abs() < f64::EPSILON);
        assert_eq!(status.online_players, 12);
        assert_eq!(status.max_players, 200);
    }

    #[tokio::test]
    async fn test_turbo_pre_warm_hand_over() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut reader = BufReader::new(&mut socket);
                let mut line = String::new();
                let _ = reader.read_line(&mut line).await;

                if line.starts_with("player-expect") {
                    let _ = socket.write_all(b"{\"status\":\"ok\"}\n").await;
                } else if line.starts_with("player-confirm") {
                    let _ = socket.write_all(b"{\"status\":\"confirmed\"}\n").await;
                }
            }
        });

        let client = HorizonTurboClient::new(TurboEndpoint::Tcp(addr));
        assert!(
            client
                .notify_player_expect("Notch", 0x1234, Some("world"))
                .await
                .is_ok()
        );
        assert!(client.notify_player_confirm("Notch").await.is_ok());
    }

    #[tokio::test]
    async fn test_turbo_rejects_injected_parameters() {
        let client =
            HorizonTurboClient::new(TurboEndpoint::Tcp("127.0.0.1:40000".parse().unwrap()));
        let err1 = client
            .notify_player_expect("bot\nquit\n", 0x1234, None)
            .await
            .unwrap_err();
        assert!(matches!(err1, TurboError::InvalidParameter(_)));

        let err2 = client
            .notify_player_expect("Steve", 0x1234, Some("world\nstop\n"))
            .await
            .unwrap_err();
        assert!(matches!(err2, TurboError::InvalidParameter(_)));

        let err3 = client
            .notify_player_confirm("user\r\nkick *")
            .await
            .unwrap_err();
        assert!(matches!(err3, TurboError::InvalidParameter(_)));
    }
}
