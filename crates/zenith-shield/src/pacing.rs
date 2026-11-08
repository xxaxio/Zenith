//! Continuous Token-Bucket Packet Rate Limiter (PPS Pacing Meter)
//!
//! Enforces zero-allocation rate limiting on individual client connections.
//! Uses a high-capacity Token Bucket that naturally absorbs network lag
//! freezes (such as mobile tethering or satellite jitter) while rigorously
//! throttling sustained bot floods and packet spam.

use std::time::Instant;
use thiserror::Error;

/// Pacing and packet rate errors.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PpsError {
    #[error(
        "Packet rate limit exceeded (bucket capacity {capacity} exhausted, attempted packet when tokens = {tokens})"
    )]
    RateExceeded { tokens: i32, capacity: u32 },
}

/// Configuration for the per-connection continuous token bucket rate limiter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PpsConfig {
    /// Continuous token generation rate per second.
    /// A legitimate Minecraft client ticks at 20 Hz (max 20 PPS in play/limbo).
    /// Default: 35.0 tokens/sec (a generous 75% overhead over the vanilla 20 Hz tickrate).
    pub refill_rate: f64,
    /// Maximum bucket capacity (maximum burst accumulation).
    /// Accommodates up to 10 seconds of accumulated lag freeze (200 packets) without false positives.
    /// Default: 200.0 tokens.
    pub burst_capacity: f64,
}

impl Default for PpsConfig {
    fn default() -> Self {
        Self {
            refill_rate: 35.0,
            burst_capacity: 200.0,
        }
    }
}

/// A zero-allocation, continuous token-bucket packet rate limiter.
#[derive(Debug, Clone)]
pub struct PpsMeter {
    config: PpsConfig,
    tokens: f64,
    last_refill: Instant,
}

impl PpsMeter {
    /// Creates a new PPS meter with the given configuration.
    /// Initialized with a full bucket (burst_capacity) to allow smooth initial connection handshake.
    #[must_use]
    pub fn new(config: PpsConfig) -> Self {
        Self {
            tokens: config.burst_capacity,
            config,
            last_refill: Instant::now(),
        }
    }

    /// Records an incoming packet and evaluates token availability at the current instant.
    pub fn record_packet(&mut self) -> Result<(), PpsError> {
        self.record_packet_at(Instant::now())
    }

    /// Records an incoming packet at a specific instant (for deterministic testing).
    pub fn record_packet_at(&mut self, now: Instant) -> Result<(), PpsError> {
        let elapsed = now
            .saturating_duration_since(self.last_refill)
            .as_secs_f64();
        self.last_refill = now;

        // Refill tokens continuously based on elapsed time, capped at burst capacity
        self.tokens =
            (self.tokens + elapsed * self.config.refill_rate).min(self.config.burst_capacity);

        if self.tokens < 1.0 {
            return Err(PpsError::RateExceeded {
                tokens: self.tokens.floor() as i32,
                capacity: self.config.burst_capacity as u32,
            });
        }

        self.tokens -= 1.0;
        Ok(())
    }

    /// Returns the currently available tokens in the bucket.
    #[must_use]
    pub fn available_tokens(&self) -> f64 {
        self.tokens
    }

    /// Resets the bucket to full capacity.
    pub fn reset(&mut self) {
        self.tokens = self.config.burst_capacity;
        self.last_refill = Instant::now();
    }
}

impl Default for PpsMeter {
    fn default() -> Self {
        Self::new(PpsConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_token_bucket_legit_sustained_traffic_never_drains() {
        let mut meter = PpsMeter::default();
        let start = Instant::now();

        // Simulate 60 seconds of continuous legit gameplay at 20 PPS (1 packet every 50ms)
        for i in 0..1200 {
            let t = start + Duration::from_millis(i * 50);
            assert!(
                meter.record_packet_at(t).is_ok(),
                "Legit player at 20 PPS must never be throttled"
            );
        }
        // Since refill rate (35/s) > consumption (20/s), bucket stays full
        assert!(meter.available_tokens() >= 199.0);
    }

    #[test]
    fn test_token_bucket_extreme_lag_freeze_6_seconds_burst_passes() {
        let mut meter = PpsMeter::default();
        let start = Instant::now();

        // Player plays 1 second normally
        for i in 0..20 {
            let t = start + Duration::from_millis(i * 50);
            meter.record_packet_at(t).unwrap();
        }

        // Severe 6-second network freeze (satellite / 4G dead zone)
        let unfreeze_time = start + Duration::from_secs(7);

        // When network returns, client's TCP buffer flushes 120 packets in 1ms
        for i in 0..120 {
            let burst_t = unfreeze_time + Duration::from_micros(i * 10);
            assert!(
                meter.record_packet_at(burst_t).is_ok(),
                "Burst of 120 packets after 6s freeze must be fully accepted"
            );
        }

        // Remaining tokens should be ~ 200 - 120 = 80
        assert!(meter.available_tokens() >= 79.0);
    }

    #[test]
    fn test_token_bucket_chaotic_jitter_bursts_passes() {
        let mut meter = PpsMeter::default();
        let mut current_time = Instant::now();

        // 10 cycles of chaotic network jitter: 1.5s freeze followed by a burst of 30 packets
        for _ in 0..10 {
            current_time += Duration::from_millis(1500);
            for _ in 0..30 {
                current_time += Duration::from_micros(100);
                assert!(meter.record_packet_at(current_time).is_ok());
            }
        }
    }

    #[test]
    fn test_token_bucket_flooder_300_pps_drains_and_fails_rapidly() {
        let mut meter = PpsMeter::default();
        let start = Instant::now();

        // Bot spams at 300 PPS (1 packet every 3.33ms)
        let mut failed = false;
        let mut packets_sent = 0;

        for i in 0..500 {
            let t = start + Duration::from_micros((i * 3333) as u64);
            if let Err(PpsError::RateExceeded { .. }) = meter.record_packet_at(t) {
                failed = true;
                packets_sent = i;
                break;
            }
        }

        assert!(
            failed,
            "A 300 PPS flooder MUST be evicted when bucket drains"
        );
        // At 300 PPS net drain is ~265/s. 200 / 265 ~ 0.75s -> ~225 packets
        assert!(
            (210..=240).contains(&packets_sent),
            "Expected eviction around ~225 packets, got {}",
            packets_sent
        );
    }

    #[test]
    fn test_token_bucket_stealth_bot_50_pps_drained_and_evicted() {
        let mut meter = PpsMeter::default();
        let start = Instant::now();

        // Stealth bot sends 50 PPS (above refill of 35 PPS)
        let mut evicted = false;
        let mut packets_sent = 0;

        for i in 0..1000 {
            let t = start + Duration::from_millis(i * 20); // 50 PPS
            if let Err(PpsError::RateExceeded { .. }) = meter.record_packet_at(t) {
                evicted = true;
                packets_sent = i;
                break;
            }
        }

        assert!(
            evicted,
            "Stealth bot sending sustained 50 PPS must be evicted"
        );
        // At 50 PPS net drain is 15/s. 200 / 15 ~ 13.3s -> ~660 packets
        assert!(
            (640..=690).contains(&packets_sent),
            "Expected eviction around ~666 packets, got {}",
            packets_sent
        );
    }
}
