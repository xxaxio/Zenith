//! Configuration parameters for transit queue, sorting policies, and health-adaptive waves.

use serde::{Deserialize, Serialize};

/// Policy used to sort players sharing the same priority tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortingPolicy {
    /// Strict First-In First-Out by arrival timestamp.
    Fifo,
    /// Lowest ping players are served first (better network packets processed faster).
    PingAscending,
    /// Highest ping players served first.
    PingDescending,
}

/// Comprehensive transit and queue configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransitConfig {
    /// Intra-tier sorting policy
    pub sorting_policy: SortingPolicy,

    /// Optional arrival tolerance window in milliseconds for FIFO sorting (0 = exact timestamp, >0 = windowed)
    pub arrival_window_ms: u64,

    /// Number of players dispatched per wave when backend is healthy
    pub nominal_wave_size: usize,

    /// Interval between waves in milliseconds when backend is healthy
    pub nominal_interval_ms: u64,

    /// Target backend nominal MSPT threshold (default <= 35.0ms)
    pub target_nominal_mspt: f64,

    /// Target backend maximum acceptable MSPT before full throttling (default >= 48.0ms)
    pub target_max_mspt: f64,

    /// Target backend minimum acceptable TPS (default >= 18.5)
    pub target_min_tps: f64,

    /// Number of players dispatched per wave when backend is throttled
    pub throttle_wave_size: usize,

    /// Interval between waves in milliseconds when backend is throttled
    pub throttle_interval_ms: u64,

    /// Cooldown on failed transfers in milliseconds before retrying
    pub failure_cooldown_ms: u64,

    /// Priority weight for Staff members
    pub staff_priority_weight: u32,

    /// Priority weight for VIP members
    pub vip_priority_weight: u32,

    /// Priority weight for Standard players
    pub standard_priority_weight: u32,
}

impl Default for TransitConfig {
    fn default() -> Self {
        Self {
            sorting_policy: SortingPolicy::Fifo,
            arrival_window_ms: 0,
            nominal_wave_size: 10,
            nominal_interval_ms: 2000,
            target_nominal_mspt: 35.0,
            target_max_mspt: 48.0,
            target_min_tps: 18.5,
            throttle_wave_size: 2,
            throttle_interval_ms: 5000,
            failure_cooldown_ms: 15000,
            staff_priority_weight: 100,
            vip_priority_weight: 10,
            standard_priority_weight: 0,
        }
    }
}
