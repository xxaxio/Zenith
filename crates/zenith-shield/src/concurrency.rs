//! Connection concurrency tracking and per-IP limit enforcement.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

/// RAII guard ensuring the connection count for an IP is decremented on drop.
pub struct ConnectionGuard {
    ip: IpAddr,
    tracker: Arc<Mutex<HashMap<IpAddr, usize>>>,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = self.tracker.lock() {
            if let Some(count) = map.get_mut(&self.ip) {
                if *count > 1 {
                    *count -= 1;
                } else {
                    map.remove(&self.ip);
                }
            }
        }
    }
}

/// Tracks and limits concurrent TCP connections per IP address.
#[derive(Debug, Clone)]
pub struct ConnectionTracker {
    max_connections_per_ip: usize,
    counts: Arc<Mutex<HashMap<IpAddr, usize>>>,
}

impl ConnectionTracker {
    pub fn new(max_connections_per_ip: usize) -> Self {
        Self {
            max_connections_per_ip,
            counts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Attempts to register a new active connection for the specified IP.
    /// Returns a `ConnectionGuard` on success, or `ShieldError::TooManyConnectionsPerIp`.
    pub fn acquire(&self, ip: IpAddr) -> Result<ConnectionGuard, crate::ShieldError> {
        let mut map = self
            .counts
            .lock()
            .map_err(|_| std::io::Error::other("lock poisoned"))?;

        let current = map.entry(ip).or_insert(0);
        if *current >= self.max_connections_per_ip {
            return Err(crate::ShieldError::TooManyConnectionsPerIp(
                ip,
                self.max_connections_per_ip,
            ));
        }

        *current += 1;
        Ok(ConnectionGuard {
            ip,
            tracker: Arc::clone(&self.counts),
        })
    }

    /// Current count of active connections for an IP.
    pub fn count_for(&self, ip: &IpAddr) -> usize {
        self.counts
            .lock()
            .ok()
            .and_then(|map| map.get(ip).copied())
            .unwrap_or(0)
    }

    /// Total number of distinct active IP addresses tracked.
    pub fn total_ips(&self) -> usize {
        self.counts.lock().ok().map(|m| m.len()).unwrap_or(0)
    }

    /// Total number of concurrent connections across all IPs.
    pub fn total_active_connections(&self) -> usize {
        self.counts
            .lock()
            .ok()
            .map(|m| m.values().sum())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_concurrency_limiting_and_raii_guard() {
        let tracker = ConnectionTracker::new(2);
        let ip: IpAddr = "192.168.1.100".parse().unwrap();

        let guard1 = tracker.acquire(ip).expect("first connection allowed");
        assert_eq!(tracker.count_for(&ip), 1);

        let guard2 = tracker.acquire(ip).expect("second connection allowed");
        assert_eq!(tracker.count_for(&ip), 2);

        // Third connection from same IP must be rejected
        let guard3 = tracker.acquire(ip);
        assert!(guard3.is_err());
        assert_eq!(tracker.count_for(&ip), 2);

        // Drop one connection
        drop(guard1);
        assert_eq!(tracker.count_for(&ip), 1);

        // Now third can acquire
        let _guard3_ok = tracker.acquire(ip).expect("now allowed after drop");
        assert_eq!(tracker.count_for(&ip), 2);

        drop(guard2);
        drop(_guard3_ok);
        assert_eq!(tracker.count_for(&ip), 0);
    }
}
