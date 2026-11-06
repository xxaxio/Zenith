//! Queue priority tier representations.

use serde::{Deserialize, Serialize};

/// Priority tier assigned to a queued player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QueuePriority {
    /// Standard player (default tier, weight 0)
    Standard,
    /// VIP / Donor player (elevated priority, weight 10)
    Vip,
    /// Staff / Moderator / Admin (highest priority, weight 100)
    Staff,
    /// Custom weighted priority level
    Custom(u32),
}

impl QueuePriority {
    /// Returns the integer weight associated with the priority.
    #[must_use]
    pub const fn weight(&self) -> u32 {
        match self {
            Self::Standard => 0,
            Self::Vip => 10,
            Self::Staff => 100,
            Self::Custom(w) => *w,
        }
    }
}

impl PartialOrd for QueuePriority {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for QueuePriority {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.weight().cmp(&other.weight())
    }
}

impl From<u32> for QueuePriority {
    fn from(val: u32) -> Self {
        match val {
            0 => Self::Standard,
            10 => Self::Vip,
            100 => Self::Staff,
            other => Self::Custom(other),
        }
    }
}
