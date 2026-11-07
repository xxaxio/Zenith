//! Health state definitions and telemetry report structures for monitored backends.

use serde::{Deserialize, Serialize};

/// High-level health status of a monitored backend game server (e.g. Horizon/Meridian).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendHealth {
    /// Server is reachable, responding to SLP pings within nominal latency.
    Healthy {
        /// Round-trip latency in milliseconds.
        latency_ms: u32,
    },
    /// Server is responding, but latency exceeds degraded threshold or ping was slow.
    Degraded {
        /// Round-trip latency in milliseconds.
        latency_ms: u32,
        /// Description of degradation condition.
        reason: String,
    },
    /// Server TCP connection failed or timed out.
    Offline {
        /// Number of consecutive probe failures.
        consecutive_failures: u32,
        /// Last observed network error message.
        last_error: String,
    },
}

impl BackendHealth {
    /// Returns true if the backend is currently considered capable of accepting players.
    #[must_use]
    pub fn is_available(&self) -> bool {
        match self {
            Self::Healthy { .. } | Self::Degraded { .. } => true,
            Self::Offline { .. } => false,
        }
    }

    /// Latency in milliseconds if available.
    #[must_use]
    pub fn latency_ms(&self) -> Option<u32> {
        match self {
            Self::Healthy { latency_ms } | Self::Degraded { latency_ms, .. } => Some(*latency_ms),
            Self::Offline { .. } => None,
        }
    }
}

/// A structured report produced after probing a backend with Minecraft SLP (Server List Ping).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeReport {
    /// Whether the server accepted the TCP connection and responded to ping.
    pub is_online: bool,
    /// Round-trip latency measured from connection establishment to status response.
    pub latency_ms: u32,
    /// Protocol version reported in the SLP JSON response (e.g. 777).
    pub protocol_version: Option<i32>,
    /// Version name reported (e.g. "26.3" or "Horizon 26.3").
    pub version_name: Option<String>,
    /// Number of online players currently on the target backend.
    pub online_players: Option<u32>,
    /// Maximum player capacity reported by the target backend.
    pub max_players: Option<u32>,
    /// Raw MOTD / description.
    pub motd: Option<String>,
    /// Error message if probe failed.
    pub error: Option<String>,
}

impl ProbeReport {
    /// Creates a report representing a failed probe attempt.
    #[must_use]
    pub fn failure(err: impl Into<String>) -> Self {
        Self {
            is_online: false,
            latency_ms: 0,
            protocol_version: None,
            version_name: None,
            online_players: None,
            max_players: None,
            motd: None,
            error: Some(err.into()),
        }
    }

    /// Creates a successful probe report.
    #[must_use]
    pub fn success(
        latency_ms: u32,
        protocol: Option<i32>,
        version: Option<String>,
        online: Option<u32>,
        max: Option<u32>,
        motd: Option<String>,
    ) -> Self {
        Self {
            is_online: true,
            latency_ms,
            protocol_version: protocol,
            version_name: version,
            online_players: online,
            max_players: max,
            motd,
            error: None,
        }
    }
}

/// Prober & Lifeline configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotemConfig {
    /// Interval between background health probes in milliseconds (default: 1000 ms).
    pub probe_interval_ms: u64,
    /// Connection and read timeout for each SLP ping in milliseconds (default: 600 ms).
    pub probe_timeout_ms: u64,
    /// Latency threshold in milliseconds above which health becomes Degraded (default: 200 ms).
    pub degraded_latency_ms: u32,
    /// Consecutive failures required before declaring target server Offline (default: 2).
    pub min_failures_for_offline: u32,
    /// Consecutive successful pings required before declaring target server Recovered (default: 2).
    pub min_successes_for_recovery: u32,
}

impl Default for TotemConfig {
    fn default() -> Self {
        Self {
            probe_interval_ms: 1000,
            probe_timeout_ms: 600,
            degraded_latency_ms: 200,
            min_failures_for_offline: 2,
            min_successes_for_recovery: 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backend_health_availability() {
        let healthy = BackendHealth::Healthy { latency_ms: 15 };
        assert!(healthy.is_available());
        assert_eq!(healthy.latency_ms(), Some(15));

        let degraded = BackendHealth::Degraded {
            latency_ms: 350,
            reason: "High latency".into(),
        };
        assert!(degraded.is_available());
        assert_eq!(degraded.latency_ms(), Some(350));

        let offline = BackendHealth::Offline {
            consecutive_failures: 3,
            last_error: "Connection refused".into(),
        };
        assert!(!offline.is_available());
        assert_eq!(offline.latency_ms(), None);
    }
}
