//! Defensive cybersecurity layer: anti-Slowloris protection, strict frame bounds,
//! token-bucket IP/CIDR rate limiting, connection concurrency limits, and protocol challenge.

pub mod challenge;
pub mod concurrency;
pub mod fingerprint;
pub mod firewall;
pub mod pacing;
pub mod rate_limit;

pub use challenge::{ChallengeStatus, ProtocolChallenge};
pub use concurrency::{ConnectionGuard, ConnectionTracker};
pub use fingerprint::{
    ClientFingerprint, FingerprintConfig, FingerprintError, FingerprintValidator,
};
pub use firewall::{
    FirewallBackend, FirewallError, FirewallManager, HardViolationReason, IpsetBackend,
    NftablesBackend, NoopFirewall, WindowsNetshBackend,
};
pub use pacing::{PpsConfig, PpsError, PpsMeter};
pub use rate_limit::{IpRateLimiter, JailManager, RateLimitConfig, SubnetKey, TokenBucket};

use std::net::IpAddr;
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use zenith_wire::{PacketBatchWriter, WireError, read_raw_packet, write_raw_packet};

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
    #[error("Packet rate limit exceeded: {0}")]
    Pacing(#[from] PpsError),
    #[error("Malformed frame or wire error: {0}")]
    Wire(#[from] WireError),
    #[error("Underlying IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// State-specific protocol phases for granular frame size and timeout validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolPhase {
    Handshake,
    Status,
    Login,
    Configuration,
    Play,
}

impl From<bool> for ProtocolPhase {
    fn from(is_play_state: bool) -> Self {
        if is_play_state {
            ProtocolPhase::Play
        } else {
            ProtocolPhase::Configuration
        }
    }
}

/// Configuration limits for socket and frame protection.
#[derive(Debug, Clone)]
pub struct ShieldLimits {
    pub max_handshake_payload: usize,
    pub max_status_payload: usize,
    pub max_login_payload: usize,
    pub max_configuration_payload: usize,
    pub max_play_payload: usize,
    pub handshake_timeout: Duration,
    pub play_idle_timeout: Duration,
    pub write_timeout: Duration,
}

impl Default for ShieldLimits {
    fn default() -> Self {
        Self {
            max_handshake_payload: 512,        // 512 B max handshake
            max_status_payload: 128,           // 128 B max status/ping
            max_login_payload: 2_048,          // 2 KiB max login
            max_configuration_payload: 32_768, // 32 KiB max configuration
            max_play_payload: 16_384,          // 16 KiB max play/limbo payload
            handshake_timeout: Duration::from_secs(15),
            play_idle_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(5),
        }
    }
}

impl ShieldLimits {
    /// Returns the maximum allowed payload for pre-play configuration phases.
    #[must_use]
    pub fn max_pre_play_payload(&self) -> usize {
        self.max_configuration_payload
    }
}

/// Evaluates whether an incoming payload length is acceptable for the given protocol phase.
#[must_use]
pub fn is_payload_safe(len: usize, limits: &ShieldLimits, phase: impl Into<ProtocolPhase>) -> bool {
    let bound = match phase.into() {
        ProtocolPhase::Handshake => limits.max_handshake_payload,
        ProtocolPhase::Status => limits.max_status_payload,
        ProtocolPhase::Login => limits.max_login_payload,
        ProtocolPhase::Configuration => limits.max_configuration_payload,
        ProtocolPhase::Play => limits.max_play_payload,
    };
    len <= bound
}

/// Shielded stream wrapper enforcing Anti-Slowloris read deadlines and frame bounds.
pub struct ShieldedStream<S> {
    stream: S,
    limits: ShieldLimits,
    batch: PacketBatchWriter,
}

impl<S: AsyncRead + AsyncWrite + Unpin> ShieldedStream<S> {
    pub fn new(stream: S, limits: ShieldLimits) -> Self {
        Self {
            stream,
            limits,
            batch: PacketBatchWriter::default(),
        }
    }

    /// Creates a shielded stream with customized packet batch capacity thresholds.
    pub fn with_batch_limits(
        stream: S,
        limits: ShieldLimits,
        max_batch_packets: usize,
        max_batch_bytes: usize,
    ) -> Self {
        Self {
            stream,
            limits,
            batch: PacketBatchWriter::new(max_batch_packets, max_batch_bytes),
        }
    }

    /// Appends a packet to the outgoing batch buffer without immediate socket write.
    /// Returns `true` if the batch has reached capacity threshold and should be flushed.
    pub fn queue_packet(&mut self, packet_id: i32, payload: &[u8]) -> bool {
        self.batch.push(packet_id, payload)
    }

    /// Appends raw pre-framed bytes to the outgoing batch buffer.
    /// Returns `true` if the batch has reached capacity threshold and should be flushed.
    pub fn queue_raw_bytes(&mut self, bytes: &[u8]) -> bool {
        self.batch.push_raw(bytes)
    }

    /// Flushes all queued batch packets in a single `write_all` syscall bounded by write_timeout.
    pub async fn flush_batch(&mut self) -> Result<usize, ShieldError> {
        if self.batch.is_empty() {
            return Ok(0);
        }
        let timeout = self.limits.write_timeout;
        let batch = &mut self.batch;
        let stream = &mut self.stream;
        match tokio::time::timeout(timeout, batch.flush(stream)).await {
            Ok(Ok(n)) => Ok(n),
            Ok(Err(e)) => Err(ShieldError::Wire(e)),
            Err(_) => Err(ShieldError::SlowlorisTimeout(timeout)),
        }
    }

    /// Returns `true` if the batch write buffer is currently empty.
    pub fn is_batch_empty(&self) -> bool {
        self.batch.is_empty()
    }

    /// Returns the number of packets queued in the batch write buffer.
    pub fn batch_packet_count(&self) -> usize {
        self.batch.packet_count()
    }

    /// Reads a packet frame with strict anti-Slowloris timeout and state-specific size validation.
    pub async fn read_packet(
        &mut self,
        phase: impl Into<ProtocolPhase>,
    ) -> Result<(i32, Vec<u8>), ShieldError> {
        let phase = phase.into();
        let (timeout, max_len) = match phase {
            ProtocolPhase::Handshake => (
                self.limits.handshake_timeout,
                self.limits.max_handshake_payload,
            ),
            ProtocolPhase::Status => (
                self.limits.handshake_timeout,
                self.limits.max_status_payload,
            ),
            ProtocolPhase::Login => (self.limits.handshake_timeout, self.limits.max_login_payload),
            ProtocolPhase::Configuration => (
                self.limits.handshake_timeout,
                self.limits.max_configuration_payload,
            ),
            ProtocolPhase::Play => (self.limits.play_idle_timeout, self.limits.max_play_payload),
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
        assert!(is_payload_safe(512, &limits, ProtocolPhase::Handshake));
        assert!(!is_payload_safe(513, &limits, ProtocolPhase::Handshake));
        assert!(is_payload_safe(128, &limits, ProtocolPhase::Status));
        assert!(!is_payload_safe(129, &limits, ProtocolPhase::Status));
        assert!(is_payload_safe(2048, &limits, ProtocolPhase::Login));
        assert!(!is_payload_safe(2049, &limits, ProtocolPhase::Login));
        assert!(is_payload_safe(
            32_768,
            &limits,
            ProtocolPhase::Configuration
        ));
        assert!(!is_payload_safe(
            32_769,
            &limits,
            ProtocolPhase::Configuration
        ));
        assert!(is_payload_safe(16_384, &limits, ProtocolPhase::Play));
        assert!(!is_payload_safe(16_385, &limits, ProtocolPhase::Play));
    }

    #[tokio::test]
    async fn test_state_specific_packet_size_rejection() {
        let (mut client, server) = duplex(1024);
        let mut shielded_server = ShieldedStream::new(server, ShieldLimits::default());

        // Send a 600-byte packet during Handshake (limit is 512)
        let large_handshake = vec![0x42u8; 600];
        write_raw_packet(&mut client, 0x00, &large_handshake)
            .await
            .unwrap();

        let res = shielded_server.read_packet(ProtocolPhase::Handshake).await;
        assert!(matches!(res, Err(ShieldError::PayloadTooLarge(601, 512))));
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

    #[tokio::test]
    async fn test_shielded_stream_batch_flush() {
        let (mut client, server) = duplex(4096);
        let mut shielded_server = ShieldedStream::new(server, ShieldLimits::default());

        assert!(shielded_server.is_batch_empty());
        shielded_server.queue_packet(0x11, b"Payload One");
        shielded_server.queue_packet(0x22, b"Payload Two");
        assert_eq!(shielded_server.batch_packet_count(), 2);
        assert!(!shielded_server.is_batch_empty());

        let flushed = shielded_server.flush_batch().await.unwrap();
        assert!(flushed > 0);
        assert!(shielded_server.is_batch_empty());

        // Read back on client side
        let (id1, p1) = read_raw_packet(&mut client, 1024).await.unwrap();
        assert_eq!(id1, 0x11);
        assert_eq!(&p1, b"Payload One");

        let (id2, p2) = read_raw_packet(&mut client, 1024).await.unwrap();
        assert_eq!(id2, 0x22);
        assert_eq!(&p2, b"Payload Two");
    }
}
