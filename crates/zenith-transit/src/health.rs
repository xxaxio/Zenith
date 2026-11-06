//! Target server health monitoring and dynamic load adaptation metrics.

use std::time::Instant;

use crate::config::TransitConfig;

/// Real-time health metrics of a destination backend server (e.g. Horizon server).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TargetServerHealth {
    /// Server Ticks Per Second (nominal: 20.0)
    pub tps: f64,
    /// Milliseconds Per Tick (nominal: <= 50.0 ms; ideal: < 35.0 ms)
    pub mspt: f64,
    /// Whether the server socket and ping are reachable
    pub is_online: bool,
    /// Whether the server has announced a draining state (e.g. preparing for restart)
    pub is_draining: bool,
    /// Timestamp of last health sample
    pub last_update: Instant,
}

impl Default for TargetServerHealth {
    fn default() -> Self {
        Self {
            tps: 20.0,
            mspt: 20.0,
            is_online: true,
            is_draining: false,
            last_update: Instant::now(),
        }
    }
}

impl TargetServerHealth {
    /// Creates a healthy initial state for a target server.
    #[must_use]
    pub fn new_online() -> Self {
        Self::default()
    }

    /// Creates an offline state for a target server.
    #[must_use]
    pub fn new_offline() -> Self {
        Self {
            tps: 0.0,
            mspt: 0.0,
            is_online: false,
            is_draining: false,
            last_update: Instant::now(),
        }
    }

    /// Checks if the target server is operating at nominal performance.
    #[must_use]
    pub fn is_nominal(&self, config: &TransitConfig) -> bool {
        self.is_online
            && !self.is_draining
            && self.tps >= config.target_min_tps
            && self.mspt <= config.target_nominal_mspt
    }

    /// Checks if the target server requires throttling due to elevated MSPT or low TPS.
    #[must_use]
    pub fn is_throttled(&self, config: &TransitConfig) -> bool {
        !self.is_nominal(config) && self.is_online && !self.is_draining
    }

    /// Computes a continuous throttle factor between 0.0 (fully healthy) and 1.0 (fully choked).
    #[must_use]
    pub fn throttle_factor(&self, config: &TransitConfig) -> f64 {
        if !self.is_online || self.is_draining {
            return 1.0;
        }
        if self.mspt <= config.target_nominal_mspt {
            return 0.0;
        }
        if self.mspt >= config.target_max_mspt {
            return 1.0;
        }

        let span = config.target_max_mspt - config.target_nominal_mspt;
        if span <= 0.0 {
            1.0
        } else {
            ((self.mspt - config.target_nominal_mspt) / span).clamp(0.0, 1.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_evaluation() {
        let config = TransitConfig::default();
        let healthy = TargetServerHealth {
            tps: 20.0,
            mspt: 25.0,
            is_online: true,
            is_draining: false,
            last_update: Instant::now(),
        };
        assert!(healthy.is_nominal(&config));
        assert!(!healthy.is_throttled(&config));
        assert_eq!(healthy.throttle_factor(&config), 0.0);

        let lagging = TargetServerHealth {
            tps: 18.0,
            mspt: 55.0,
            is_online: true,
            is_draining: false,
            last_update: Instant::now(),
        };
        assert!(!lagging.is_nominal(&config));
        assert!(lagging.is_throttled(&config));
        assert_eq!(lagging.throttle_factor(&config), 1.0);
    }
}
