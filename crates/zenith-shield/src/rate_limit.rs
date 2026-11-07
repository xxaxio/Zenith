//! Token-bucket rate limiting and temporary IP jailing (anti-botnet defense).

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::{Duration, Instant};

/// Token-bucket algorithm for burst-tolerant rate limiting.
#[derive(Debug, Clone)]
pub struct TokenBucket {
    tokens: f64,
    capacity: f64,
    fill_rate: f64, // tokens per second
    last_refreshed: Instant,
}

impl TokenBucket {
    pub fn new(capacity: f64, fill_rate: f64, now: Instant) -> Self {
        Self {
            tokens: capacity,
            capacity,
            fill_rate,
            last_refreshed: now,
        }
    }

    /// Attempts to consume 1 token. Returns true if allowed, false if exhausted.
    pub fn try_consume(&mut self, now: Instant) -> bool {
        let elapsed = now.duration_since(self.last_refreshed).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.fill_rate).min(self.capacity);
        self.last_refreshed = now;

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Normalized subnet key for CIDR aggregation (/24 for IPv4, /64 for IPv6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubnetKey {
    V4(u32),
    V6(u64),
}

impl SubnetKey {
    pub fn from_ip(ip: &IpAddr) -> Self {
        match ip {
            IpAddr::V4(v4) => {
                let octets = v4.octets();
                // /24 mask: keep first 3 octets
                let prefix = (u32::from(octets[0]) << 24)
                    | (u32::from(octets[1]) << 16)
                    | (u32::from(octets[2]) << 8);
                SubnetKey::V4(prefix)
            }
            IpAddr::V6(v6) => {
                let octets = v6.octets();
                // /64 mask: keep first 8 octets
                let mut buf = [0u8; 8];
                buf.copy_from_slice(&octets[..8]);
                SubnetKey::V6(u64::from_be_bytes(buf))
            }
        }
    }

    pub fn to_cidr_string(&self) -> String {
        match self {
            SubnetKey::V4(prefix) => {
                let o1 = (prefix >> 24) & 0xFF;
                let o2 = (prefix >> 16) & 0xFF;
                let o3 = (prefix >> 8) & 0xFF;
                format!("{o1}.{o2}.{o3}.0/24")
            }
            SubnetKey::V6(prefix) => {
                format!("{:016x}::/64", prefix)
            }
        }
    }
}

/// Dynamic IP Jailing / Blacklisting manager.
#[derive(Debug, Clone, Default)]
pub struct JailManager {
    jailed: HashMap<IpAddr, Instant>,
}

impl JailManager {
    pub fn new() -> Self {
        Self {
            jailed: HashMap::new(),
        }
    }

    /// Returns true if the IP is currently jailed.
    pub fn is_jailed(&self, ip: &IpAddr, now: Instant) -> bool {
        if let Some(&expires_at) = self.jailed.get(ip) {
            now < expires_at
        } else {
            false
        }
    }

    /// Remaining jail duration for an IP, if jailed.
    pub fn remaining_jail_time(&self, ip: &IpAddr, now: Instant) -> Option<Duration> {
        self.jailed.get(ip).and_then(|&exp| {
            if exp > now {
                Some(exp.duration_since(now))
            } else {
                None
            }
        })
    }

    /// Puts an IP in jail until `now + duration`.
    pub fn jail(&mut self, ip: IpAddr, now: Instant, duration: Duration) {
        let expires_at = now + duration;
        self.jailed.insert(ip, expires_at);
    }

    /// Pardons an IP from jail.
    pub fn pardon(&mut self, ip: &IpAddr) {
        self.jailed.remove(ip);
    }

    /// Prunes expired jail records to avoid memory growth.
    pub fn prune_expired(&mut self, now: Instant) {
        self.jailed.retain(|_, &mut exp| exp > now);
    }

    /// Current count of jailed IPs.
    pub fn count(&self) -> usize {
        self.jailed.len()
    }
}

/// Comprehensive rate-limiting configuration.
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    pub ip_rate_limit: f64,         // max tokens/sec per IP
    pub ip_burst_capacity: f64,     // max burst capacity per IP
    pub subnet_rate_limit: f64,     // max tokens/sec per /24 or /64 subnet
    pub subnet_burst_capacity: f64, // max burst capacity per subnet
    pub violation_threshold: u32,   // violations before automatic jail
    pub jail_duration: Duration,    // duration of temporary ban
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            ip_rate_limit: 2.0,
            ip_burst_capacity: 5.0,
            subnet_rate_limit: 10.0,
            subnet_burst_capacity: 25.0,
            violation_threshold: 5,
            jail_duration: Duration::from_secs(900), // 15 minutes default
        }
    }
}

/// Hierarchical rate limiter guarding IPs and CIDR subnets against floods.
#[derive(Debug)]
pub struct IpRateLimiter {
    config: RateLimitConfig,
    ip_buckets: HashMap<IpAddr, TokenBucket>,
    subnet_buckets: HashMap<SubnetKey, TokenBucket>,
    violations: HashMap<IpAddr, u32>,
    jail: JailManager,
    whitelist: HashSet<IpAddr>,
}

impl IpRateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        let mut whitelist = HashSet::new();
        whitelist.insert(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
        whitelist.insert(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1)));

        Self {
            config,
            ip_buckets: HashMap::new(),
            subnet_buckets: HashMap::new(),
            violations: HashMap::new(),
            jail: JailManager::new(),
            whitelist,
        }
    }

    /// Adds an IP to the bypass whitelist.
    pub fn add_whitelist(&mut self, ip: IpAddr) {
        self.whitelist.insert(ip);
    }

    /// Checks whether an incoming IP is whitelisted.
    pub fn is_whitelisted(&self, ip: &IpAddr) -> bool {
        self.whitelist.contains(ip)
    }

    /// Evaluates if an incoming connection or handshake is permitted.
    ///
    /// Returns:
    /// - `Ok(())` if allowed.
    /// - `Err(ShieldError::IpJailed)` if already jailed.
    /// - `Err(ShieldError::RateLimitExceeded)` if IP token bucket exhausted.
    /// - `Err(ShieldError::SubnetRateLimitExceeded)` if subnet token bucket exhausted.
    pub fn check_allowed(&mut self, ip: IpAddr, now: Instant) -> Result<(), crate::ShieldError> {
        // Whitelisted IPs are always allowed
        if self.is_whitelisted(&ip) {
            return Ok(());
        }

        // 1. Check if currently in jail
        if self.jail.is_jailed(&ip, now) {
            let remaining = self
                .jail
                .remaining_jail_time(&ip, now)
                .unwrap_or(self.config.jail_duration);
            return Err(crate::ShieldError::IpJailed(ip, remaining));
        }

        // 2. Check IP Token Bucket
        let ip_allowed = {
            let config = &self.config;
            let bucket = self.ip_buckets.entry(ip).or_insert_with(|| {
                TokenBucket::new(config.ip_burst_capacity, config.ip_rate_limit, now)
            });
            bucket.try_consume(now)
        };

        if !ip_allowed {
            let v = self.violations.entry(ip).or_insert(0);
            *v += 1;
            if *v >= self.config.violation_threshold {
                self.jail.jail(ip, now, self.config.jail_duration);
                return Err(crate::ShieldError::IpJailed(ip, self.config.jail_duration));
            }
            return Err(crate::ShieldError::RateLimitExceeded(ip));
        }

        // 3. Check Subnet Token Bucket
        let subnet_key = SubnetKey::from_ip(&ip);
        let subnet_allowed = {
            let config = &self.config;
            let bucket = self.subnet_buckets.entry(subnet_key).or_insert_with(|| {
                TokenBucket::new(config.subnet_burst_capacity, config.subnet_rate_limit, now)
            });
            bucket.try_consume(now)
        };

        if !subnet_allowed {
            return Err(crate::ShieldError::SubnetRateLimitExceeded(
                subnet_key.to_cidr_string(),
            ));
        }

        Ok(())
    }

    /// Punishes an IP directly (e.g. following bad frame attack).
    pub fn punish(&mut self, ip: IpAddr, now: Instant) {
        if !self.is_whitelisted(&ip) {
            self.jail.jail(ip, now, self.config.jail_duration);
        }
    }

    /// Access jail manager.
    pub fn jail_manager(&self) -> &JailManager {
        &self.jail
    }

    /// Mutable access to jail manager.
    pub fn jail_manager_mut(&mut self) -> &mut JailManager {
        &mut self.jail
    }

    /// Prunes idle buckets and expired jail entries to keep RAM sub-megabyte.
    pub fn prune(&mut self, now: Instant) {
        self.jail.prune_expired(now);
        // Retain only buckets modified in the last 10 minutes
        self.ip_buckets
            .retain(|_, b| now.duration_since(b.last_refreshed) < Duration::from_secs(600));
        self.subnet_buckets
            .retain(|_, b| now.duration_since(b.last_refreshed) < Duration::from_secs(600));
        self.violations.retain(|ip, _| self.jail.is_jailed(ip, now));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_bucket_consume_and_refill() {
        let now = Instant::now();
        let mut bucket = TokenBucket::new(3.0, 1.0, now);

        assert!(bucket.try_consume(now));
        assert!(bucket.try_consume(now));
        assert!(bucket.try_consume(now));
        assert!(!bucket.try_consume(now)); // exhausted

        // Simulate 1.5 seconds later
        let later = now + Duration::from_millis(1500);
        assert!(bucket.try_consume(later));
        assert!(!bucket.try_consume(later));
    }

    #[test]
    fn test_subnet_key_generation() {
        let ip_v4: IpAddr = "192.168.1.42".parse().unwrap();
        let key_v4 = SubnetKey::from_ip(&ip_v4);
        assert_eq!(key_v4.to_cidr_string(), "192.168.1.0/24");

        let ip_v6: IpAddr = "2001:db8:85a3::8a2e:370:7334".parse().unwrap();
        let key_v6 = SubnetKey::from_ip(&ip_v6);
        assert!(key_v6.to_cidr_string().ends_with("::/64"));
    }

    #[test]
    fn test_jail_lifecycle() {
        let now = Instant::now();
        let mut jail = JailManager::new();
        let ip: IpAddr = "10.0.0.1".parse().unwrap();

        assert!(!jail.is_jailed(&ip, now));
        jail.jail(ip, now, Duration::from_secs(300));
        assert!(jail.is_jailed(&ip, now));
        assert!(jail.remaining_jail_time(&ip, now).unwrap().as_secs() > 290);

        // After expiration
        let later = now + Duration::from_secs(301);
        assert!(!jail.is_jailed(&ip, later));
    }
}
