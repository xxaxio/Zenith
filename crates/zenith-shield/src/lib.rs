//! Defensive cybersecurity layer: anti-Slowloris protection, strict frame bounds,
//! token-bucket IP/CIDR rate limiting, connection concurrency limits, and protocol challenge.

pub mod challenge;
pub mod concurrency;
pub mod fingerprint;
pub mod rate_limit;

pub use challenge::{ChallengeStatus, ProtocolChallenge};
pub use concurrency::{ConnectionGuard, ConnectionTracker};
pub use fingerprint::{
    ClientFingerprint, FingerprintConfig, FingerprintError, FingerprintValidator,
};
pub use rate_limit::{IpRateLimiter, JailManager, RateLimitConfig, SubnetKey, TokenBucket};

use std::net::IpAddr;
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use zenith_wire::{WireError, read_raw_packet, write_raw_packet};

/// Defensive shield errors.
#[derive(Debug, Error)]
pub enum ShieldError {
    #[error("Slowloris timeout exceeded ({0:?})")]
    SlowlorisTimeout(Duration),
    #[error("Frame size exceeded bound: {0} bytes (max: {1})")]
    PayloadTooLarge(usize, usize),
    #[error("Rate limit exceeded for IP {0}")]
    RateLimitExceeded(IpAddr),
    #[error("Subnet rate limit exceeded for CIDR {0}")]
    SubnetRateLimitExceeded(String),
    #[error("Too many concurrent connections from IP {0} (max: {1})")]
    TooManyConnectionsPerIp(IpAddr, usize),
    #[error("IP {0} is jailed for remaining {1:?}")]
    IpJailed(IpAddr, Duration),
    #[error("Protocol challenge timeout exceeded ({0:?})")]
    ChallengeTimeout(Duration),
    #[error("Client fingerprint validation failed: {0}")]
    Fingerprint(#[from] FingerprintError),
    #[error("Malformed frame or wire error: {0}")]
    Wire(#[from] WireError),
    #[error("Underlying IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Configuration limits for socket and frame protection.
#[derive(Debug, Clone)]
pub struct ShieldLimits {
    pub max_pre_play_payload: usize,
    pub max_play_payload: usize,
    pub handshake_timeout: Duration,
    pub play_idle_timeout: Duration,
    pub write_timeout: Duration,
}

impl Default for ShieldLimits {
    fn default() -> Self {
        Self {
            max_pre_play_payload: 32_768,      // 32 KiB max pre-play payload
            max_play_payload: 2 * 1024 * 1024, // 2 MiB max play payload
            handshake_timeout: Duration::from_secs(15),
            play_idle_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(5),
        }
    }
}

/// Evaluates whether an incoming payload length is acceptable.
#[must_use]
pub fn is_payload_safe(len: usize, limits: &ShieldLimits, is_play_state: bool) -> bool {
    let bound = if is_play_state {
        limits.max_play_payload
    } else {
        limits.max_pre_play_payload
    };
    len <= bound
}

/// Shielded stream wrapper enforcing Anti-Slowloris read deadlines and frame bounds.
pub struct ShieldedStream<S> {
    stream: S,
    limits: ShieldLimits,
}

impl<S: AsyncRead + AsyncWrite + Unpin> ShieldedStream<S> {
    pub fn new(stream: S, limits: ShieldLimits) -> Self {
        Self { stream, limits }
    }

    /// Reads a packet frame with strict anti-Slowloris timeout and size validation.
    pub async fn read_packet(
        &mut self,
        is_play_state: bool,
    ) -> Result<(i32, Vec<u8>), ShieldError> {
        let timeout = if is_play_state {
            self.limits.play_idle_timeout
        } else {
            self.limits.handshake_timeout
        };

        let max_len = if is_play_state {
            self.limits.max_play_payload
        } else {
            self.limits.max_pre_play_payload
        };

        let read_future = read_raw_packet(&mut self.stream, max_len);
        match tokio::time::timeout(timeout, read_future).await {
            Ok(Ok((id, payload))) => Ok((id, payload)),
            Ok(Err(WireError::PayloadTooLarge(sz))) => {
                Err(ShieldError::PayloadTooLarge(sz, max_len))
            }
            Ok(Err(other)) => Err(ShieldError::Wire(other)),
            Err(_) => Err(ShieldError::SlowlorisTimeout(timeout)),
        }
    }

    /// Writes a packet frame to the stream bounded by write_timeout to prevent Zero-Window stalls.
    pub async fn write_packet(
        &mut self,
        packet_id: i32,
        payload: &[u8],
    ) -> Result<(), ShieldError> {
        let timeout = self.limits.write_timeout;
        match tokio::time::timeout(
            timeout,
            write_raw_packet(&mut self.stream, packet_id, payload),
        )
        .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(wire_err)) => Err(ShieldError::Wire(wire_err)),
            Err(_) => Err(ShieldError::SlowlorisTimeout(timeout)),
        }
    }

    /// Writes raw pre-framed packet bytes directly to the stream bounded by write_timeout.
    pub async fn write_raw_bytes(&mut self, bytes: &[u8]) -> Result<(), ShieldError> {
        use tokio::io::AsyncWriteExt;
        let timeout = self.limits.write_timeout;
        match tokio::time::timeout(timeout, async {
            self.stream.write_all(bytes).await?;
            self.stream.flush().await?;
            Ok::<(), std::io::Error>(())
        })
        .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(io_err)) => Err(ShieldError::Io(io_err)),
            Err(_) => Err(ShieldError::SlowlorisTimeout(timeout)),
        }
    }

    /// Access the underlying stream.
    pub fn get_ref(&self) -> &S {
        &self.stream
    }

    /// Access mutable underlying stream.
    pub fn get_mut(&mut self) -> &mut S {
        &mut self.stream
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[test]
    fn test_payload_bounds() {
        let limits = ShieldLimits::default();
        assert!(is_payload_safe(1024, &limits, false));
        assert!(is_payload_safe(32_768, &limits, false));
        assert!(!is_payload_safe(32_769, &limits, false));
        assert!(is_payload_safe(1_000_000, &limits, true));
    }

    #[tokio::test]
    async fn test_shielded_stream_roundtrip() {
        let (client, server) = duplex(1024);
        let mut shielded_server = ShieldedStream::new(server, ShieldLimits::default());
        let mut client = client;

        // Write from client to server
        write_raw_packet(&mut client, 0x00, b"Handshake")
            .await
            .unwrap();

        let (id, payload) = shielded_server.read_packet(false).await.unwrap();
        assert_eq!(id, 0x00);
        assert_eq!(&payload, b"Handshake");

        // Write from server to client
        shielded_server
            .write_packet(0x01, b"Response")
            .await
            .unwrap();

        let (id2, payload2) = read_raw_packet(&mut client, 1024).await.unwrap();
        assert_eq!(id2, 0x01);
        assert_eq!(&payload2, b"Response");
    }

    #[tokio::test]
    async fn test_slowloris_timeout() {
        let (_client, server) = duplex(1024);
        let limits = ShieldLimits {
            handshake_timeout: Duration::from_millis(50),
            ..Default::default()
        };
        let mut shielded_server = ShieldedStream::new(server, limits);

        let res = shielded_server.read_packet(false).await;
        assert!(matches!(res, Err(ShieldError::SlowlorisTimeout(_))));
    }

    #[tokio::test]
    async fn test_slowloris_write_timeout() {
        // Create a duplex with tiny buffer (64 bytes) and don't read from client side
        let (_client, server) = duplex(64);
        let limits = ShieldLimits {
            write_timeout: Duration::from_millis(50),
            ..Default::default()
        };
        let mut shielded_server = ShieldedStream::new(server, limits);

        // Writing more bytes than duplex capacity will stall and trigger write_timeout
        let big_payload = vec![0u8; 1024];
        let res = shielded_server.write_raw_bytes(&big_payload).await;
        assert!(matches!(res, Err(ShieldError::SlowlorisTimeout(_))));
    }
}
