//! Crash evacuation lifeline, health poller, and automatic backend re-routing.

/// Health state of a monitored backend game server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendHealth {
    Healthy,
    Degraded,
    Offline,
}

/// A target backend server monitored for crash recovery.
#[derive(Debug, Clone)]
pub struct MonitoredBackend {
    pub address: String,
    pub status: BackendHealth,
}

impl MonitoredBackend {
    #[must_use]
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            status: BackendHealth::Offline,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backend_status() {
        let backend = MonitoredBackend::new("127.0.0.1:25566");
        assert_eq!(backend.status, BackendHealth::Offline);
    }
}
