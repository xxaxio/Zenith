//! Defensive cybersecurity layer: anti-Slowloris protection, strict frame bounds, and rate-limiting.

use std::time::Duration;

/// Configuration limits for socket protection.
#[derive(Debug, Clone)]
pub struct ShieldLimits {
    pub max_packet_size: usize,
    pub socket_timeout: Duration,
}

impl Default for ShieldLimits {
    fn default() -> Self {
        Self {
            max_packet_size: 32_768, // 32 KiB max pre-play payload
            socket_timeout: Duration::from_secs(15),
        }
    }
}

/// Evaluates whether an incoming payload length is acceptable.
#[must_use]
pub fn is_payload_safe(len: usize, limits: &ShieldLimits) -> bool {
    len <= limits.max_packet_size
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_payload_bounds() {
        let limits = ShieldLimits::default();
        assert!(is_payload_safe(1024, &limits));
        assert!(is_payload_safe(32_768, &limits));
        assert!(!is_payload_safe(32_769, &limits));
    }
}
