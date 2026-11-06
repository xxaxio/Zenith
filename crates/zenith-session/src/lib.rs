//! Zero-chunk virtual dimension session holding and connection actors.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Unique identifier for an active player session in Zenith.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(pub u64);

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

impl SessionId {
    /// Generates a new unique session identifier.
    #[must_use]
    pub fn next() -> Self {
        Self(NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// State of a player held within the virtual dimension.
#[derive(Debug)]
pub struct PlayerSession {
    pub id: SessionId,
    pub username: String,
    pub connected_at: Instant,
    pub last_keepalive_sent: Option<Instant>,
    pub ping_ms: u32,
}

impl PlayerSession {
    /// Creates a new player session.
    #[must_use]
    pub fn new(username: impl Into<String>) -> Self {
        Self {
            id: SessionId::next(),
            username: username.into(),
            connected_at: Instant::now(),
            last_keepalive_sent: None,
            ping_ms: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_creation() {
        let session = PlayerSession::new("Tester");
        assert_eq!(session.username, "Tester");
        assert!(session.id.0 > 0);
    }
}
