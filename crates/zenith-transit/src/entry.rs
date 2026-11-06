//! Queue entry state representing a player waiting for server transit.

use std::time::Instant;
use zenith_session::SessionId;

use crate::priority::QueuePriority;

/// A player entry currently waiting in the transit queue.
#[derive(Debug, Clone)]
pub struct QueueEntry {
    /// Unique connection session identifier
    pub session_id: SessionId,
    /// Player Minecraft UUID
    pub uuid: u128,
    /// Player username
    pub username: String,
    /// Assigned priority tier
    pub priority: QueuePriority,
    /// Round-trip ping in milliseconds
    pub ping_ms: u32,
    /// Timestamp when player entered the queue
    pub enqueued_at: Instant,
    /// Name or ID of the destination backend server
    pub target_server: String,
}

impl QueueEntry {
    /// Creates a new queue entry.
    #[must_use]
    pub fn new(
        session_id: SessionId,
        uuid: u128,
        username: impl Into<String>,
        priority: QueuePriority,
        ping_ms: u32,
        target_server: impl Into<String>,
    ) -> Self {
        Self {
            session_id,
            uuid,
            username: username.into(),
            priority,
            ping_ms,
            enqueued_at: Instant::now(),
            target_server: target_server.into(),
        }
    }

    /// Duration spent waiting in the queue so far.
    #[must_use]
    pub fn wait_duration(&self) -> std::time::Duration {
        self.enqueued_at.elapsed()
    }
}
