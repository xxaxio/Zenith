//! Health-adaptive wave drain scheduling and cadence calculations.

use std::time::{Duration, Instant};

use crate::config::TransitConfig;
use crate::health::TargetServerHealth;

/// Adaptive wave scheduler paced dynamically according to backend load.
#[derive(Debug, Clone, Default)]
pub struct WaveScheduler {
    last_drain: Option<Instant>,
}

impl WaveScheduler {
    /// Creates a new wave scheduler.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Computes the effective wave interval duration given target health and config.
    #[must_use]
    pub fn current_interval(
        &self,
        health: &TargetServerHealth,
        config: &TransitConfig,
    ) -> Duration {
        let factor = health.throttle_factor(config);
        let min_ms = config.nominal_interval_ms as f64;
        let max_ms = config.throttle_interval_ms as f64;
        let effective_ms = min_ms + (max_ms - min_ms) * factor;
        Duration::from_millis(effective_ms as u64)
    }

    /// Computes the maximum number of players dispatched in the current wave.
    #[must_use]
    pub fn current_wave_size(&self, health: &TargetServerHealth, config: &TransitConfig) -> usize {
        if !health.is_online || health.is_draining {
            return 0;
        }
        let factor = health.throttle_factor(config);
        let nominal = config.nominal_wave_size as f64;
        let throttle = config.throttle_wave_size as f64;
        let effective = nominal - (nominal - throttle) * factor;
        effective.round().max(1.0) as usize
    }

    /// Checks if a wave drain cycle is due for execution.
    #[must_use]
    pub fn is_due(
        &self,
        now: Instant,
        health: &TargetServerHealth,
        config: &TransitConfig,
    ) -> bool {
        if !health.is_online || health.is_draining {
            return false;
        }
        match self.last_drain {
            None => true,
            Some(last) => now.duration_since(last) >= self.current_interval(health, config),
        }
    }

    /// Records the execution timestamp of a dispatched wave.
    pub fn record_drain(&mut self, now: Instant) {
        self.last_drain = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wave_scaling() {
        let config = TransitConfig::default();
        let scheduler = WaveScheduler::new();

        // 1. Healthy backend (low MSPT) -> full nominal wave
        let healthy = TargetServerHealth {
            tps: 20.0,
            mspt: 20.0,
            is_online: true,
            is_draining: false,
            last_update: Instant::now(),
        };
        assert_eq!(scheduler.current_wave_size(&healthy, &config), 10);
        assert_eq!(
            scheduler.current_interval(&healthy, &config),
            Duration::from_millis(2000)
        );

        // 2. Choked backend (high MSPT) -> throttled wave
        let choked = TargetServerHealth {
            tps: 15.0,
            mspt: 60.0,
            is_online: true,
            is_draining: false,
            last_update: Instant::now(),
        };
        assert_eq!(scheduler.current_wave_size(&choked, &config), 2);
        assert_eq!(
            scheduler.current_interval(&choked, &config),
            Duration::from_millis(5000)
        );
    }
}
