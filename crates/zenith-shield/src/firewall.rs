//! Kernel-level packet drop and operating system firewall synchronization.
//!
//! Provides Level 2 defensive escalation by offloading undisputed malicious botnet
//! IP addresses directly to operating system packet filters (`nftables` / `ipset` on Linux,
//! `netsh advfirewall` on Windows).
//!
//! Evaluates strictly against `HardViolationReason` (zero false positives) and processes
//! all synchronization asynchronously through a dedicated background worker thread to ensure
//! zero reactor stalling or latency penalty for in-flight sessions.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, error, info, warn};

use crate::ProtocolPhase;

/// Errors emitted during OS firewall interactions.
#[derive(Debug, Error)]
pub enum FirewallError {
    #[error("Operating system command execution failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Command exited with non-zero code {code:?}: {stderr}")]
    CommandFailed { code: Option<i32>, stderr: String },
    #[error("Missing administrative privileges to manipulate network packet filters")]
    PermissionDenied,
    #[error("Firewall backend '{0}' is not supported on this platform")]
    UnsupportedPlatform(String),
}

/// The specific, undisputed malicious reason triggering Kernel Level 2 escalation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HardViolationReason {
    /// Severe frame overflow during handshake or status phase (deliberate buffer attack).
    MalformedFrameOverflow {
        phase: ProtocolPhase,
        size: usize,
        max: usize,
    },
    /// Explicitly blacklisted, known botnet software identity (e.g. mcstorm, mctool).
    BlacklistedBrand { brand: String },
    /// Massive sustained packet flood violating physical human constraints.
    ExtremePacketFlooding { burst_count: u32 },
}

impl std::fmt::Display for HardViolationReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MalformedFrameOverflow { phase, size, max } => {
                write!(
                    f,
                    "Oversized frame in {:?} phase (size: {} B, max: {} B)",
                    phase, size, max
                )
            }
            Self::BlacklistedBrand { brand } => {
                write!(f, "Blacklisted attack client brand: {:?}", brand)
            }
            Self::ExtremePacketFlooding { burst_count } => {
                write!(f, "Extreme packet flood: {} packets in burst", burst_count)
            }
        }
    }
}

/// Trait defining OS packet filtering capabilities.
pub trait FirewallBackend: Send + Sync {
    /// Directs the OS kernel to drop all incoming packets from `ip` for `duration`.
    fn ban_ip(&self, ip: IpAddr, duration: Duration) -> Result<(), FirewallError>;

    /// Removes an IP from the kernel blocklist.
    fn unban_ip(&self, ip: IpAddr) -> Result<(), FirewallError>;

    /// Flushes or initializes the Zenith firewall ruleset/table.
    fn flush(&self) -> Result<(), FirewallError>;

    /// Identifier name of the backend.
    fn name(&self) -> &'static str;
}

/// Passive / Mock firewall backend used for unit testing, dry-runs, and unprivileged hosts.
#[derive(Debug, Default)]
pub struct NoopFirewall {
    banned: std::sync::RwLock<HashMap<IpAddr, Instant>>,
}

impl NoopFirewall {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_banned(&self, ip: &IpAddr) -> bool {
        self.banned.read().unwrap().contains_key(ip)
    }

    pub fn banned_count(&self) -> usize {
        self.banned.read().unwrap().len()
    }
}

impl FirewallBackend for NoopFirewall {
    fn ban_ip(&self, ip: IpAddr, duration: Duration) -> Result<(), FirewallError> {
        let mut map = self.banned.write().unwrap();
        map.insert(ip, Instant::now() + duration);
        debug!(ip = %ip, duration_secs = duration.as_secs(), "NoopFirewall: recorded mock IP ban");
        Ok(())
    }

    fn unban_ip(&self, ip: IpAddr) -> Result<(), FirewallError> {
        self.banned.write().unwrap().remove(&ip);
        Ok(())
    }

    fn flush(&self) -> Result<(), FirewallError> {
        self.banned.write().unwrap().clear();
        Ok(())
    }

    fn name(&self) -> &'static str {
        "noop"
    }
}

/// Linux `nftables` native set backend with kernel auto-expiring timeouts.
#[derive(Debug)]
pub struct NftablesBackend {
    table: String,
    set_v4: String,
    set_v6: String,
}

impl NftablesBackend {
    pub fn new(table: &str, set_v4: &str, set_v6: &str) -> Self {
        Self {
            table: table.to_string(),
            set_v4: set_v4.to_string(),
            set_v6: set_v6.to_string(),
        }
    }

    /// Initializes the dedicated nftables table and sets if not already present.
    pub fn initialize(&self) -> Result<(), FirewallError> {
        // Create table: nft add table inet <table_name>
        let cmd = Command::new("nft")
            .args(["add", "table", "inet", &self.table])
            .output();

        match cmd {
            Ok(out) if out.status.success() => {
                // Sets with auto-timeout
                let _ = Command::new("nft")
                    .args([
                        "add",
                        "set",
                        "inet",
                        &self.table,
                        &self.set_v4,
                        "{",
                        "type",
                        "ipv4_addr;",
                        "flags",
                        "timeout;",
                        "}",
                    ])
                    .output();
                let _ = Command::new("nft")
                    .args([
                        "add",
                        "set",
                        "inet",
                        &self.table,
                        &self.set_v6,
                        "{",
                        "type",
                        "ipv6_addr;",
                        "flags",
                        "timeout;",
                        "}",
                    ])
                    .output();

                // Drop rule on input
                let _ = Command::new("nft")
                    .args([
                        "add",
                        "chain",
                        "inet",
                        &self.table,
                        "input",
                        "{",
                        "type",
                        "filter",
                        "hook",
                        "input",
                        "priority",
                        "-300;",
                        "policy",
                        "accept;",
                        "}",
                    ])
                    .output();

                let _ = Command::new("nft")
                    .args([
                        "add",
                        "rule",
                        "inet",
                        &self.table,
                        "input",
                        "ip",
                        "saddr",
                        &format!("@{}", self.set_v4),
                        "drop",
                    ])
                    .output();

                let _ = Command::new("nft")
                    .args([
                        "add",
                        "rule",
                        "inet",
                        &self.table,
                        "input",
                        "ip6",
                        "saddr",
                        &format!("@{}", self.set_v6),
                        "drop",
                    ])
                    .output();

                info!(table = %self.table, "Linux nftables Zenith drop sets successfully initialized");
                Ok(())
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                warn!(status = ?out.status.code(), stderr = %stderr, "nftables table initialization notice");
                Err(FirewallError::CommandFailed {
                    code: out.status.code(),
                    stderr,
                })
            }
            Err(e) => {
                warn!(error = %e, "nft command unavailable or lacking permissions");
                Err(FirewallError::Io(e))
            }
        }
    }
}

impl FirewallBackend for NftablesBackend {
    fn ban_ip(&self, ip: IpAddr, duration: Duration) -> Result<(), FirewallError> {
        let (set_name, ip_str) = match ip {
            IpAddr::V4(v4) => (&self.set_v4, v4.to_string()),
            IpAddr::V6(v6) => (&self.set_v6, v6.to_string()),
        };

        let timeout_str = format!("{}s", duration.as_secs());
        let element_spec = format!("{{ {} timeout {} }}", ip_str, timeout_str);

        let out = Command::new("nft")
            .args([
                "add",
                "element",
                "inet",
                &self.table,
                set_name,
                &element_spec,
            ])
            .output()?;

        if out.status.success() {
            info!(ip = %ip, duration = %timeout_str, "Kernel drop applied via nftables");
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            Err(FirewallError::CommandFailed {
                code: out.status.code(),
                stderr,
            })
        }
    }

    fn unban_ip(&self, ip: IpAddr) -> Result<(), FirewallError> {
        let (set_name, ip_str) = match ip {
            IpAddr::V4(v4) => (&self.set_v4, v4.to_string()),
            IpAddr::V6(v6) => (&self.set_v6, v6.to_string()),
        };

        let element_spec = format!("{{ {} }}", ip_str);
        let out = Command::new("nft")
            .args([
                "delete",
                "element",
                "inet",
                &self.table,
                set_name,
                &element_spec,
            ])
            .output()?;

        if out.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            Err(FirewallError::CommandFailed {
                code: out.status.code(),
                stderr,
            })
        }
    }

    fn flush(&self) -> Result<(), FirewallError> {
        let _ = Command::new("nft")
            .args(["flush", "set", "inet", &self.table, &self.set_v4])
            .output();
        let _ = Command::new("nft")
            .args(["flush", "set", "inet", &self.table, &self.set_v6])
            .output();
        Ok(())
    }

    fn name(&self) -> &'static str {
        "nftables"
    }
}

/// Linux `ipset` backend for traditional Linux deployments.
#[derive(Debug)]
pub struct IpsetBackend {
    set_name: String,
}

impl IpsetBackend {
    pub fn new(set_name: &str) -> Self {
        Self {
            set_name: set_name.to_string(),
        }
    }

    pub fn initialize(&self) -> Result<(), FirewallError> {
        let out = Command::new("ipset")
            .args([
                "create",
                &self.set_name,
                "hash:ip",
                "timeout",
                "900",
                "-exist",
            ])
            .output()?;
        if out.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            Err(FirewallError::CommandFailed {
                code: out.status.code(),
                stderr,
            })
        }
    }
}

impl FirewallBackend for IpsetBackend {
    fn ban_ip(&self, ip: IpAddr, duration: Duration) -> Result<(), FirewallError> {
        let out = Command::new("ipset")
            .args([
                "add",
                &self.set_name,
                &ip.to_string(),
                "timeout",
                &duration.as_secs().to_string(),
                "-exist",
            ])
            .output()?;
        if out.status.success() {
            info!(ip = %ip, duration_secs = duration.as_secs(), "Kernel drop applied via ipset");
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            Err(FirewallError::CommandFailed {
                code: out.status.code(),
                stderr,
            })
        }
    }

    fn unban_ip(&self, ip: IpAddr) -> Result<(), FirewallError> {
        let out = Command::new("ipset")
            .args(["del", &self.set_name, &ip.to_string(), "-exist"])
            .output()?;
        if out.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            Err(FirewallError::CommandFailed {
                code: out.status.code(),
                stderr,
            })
        }
    }

    fn flush(&self) -> Result<(), FirewallError> {
        let _ = Command::new("ipset")
            .args(["flush", &self.set_name])
            .output();
        Ok(())
    }

    fn name(&self) -> &'static str {
        "ipset"
    }
}

/// Matches an IP address against exact IP addresses or CIDR blocks (e.g. `10.0.0.0/8`, `172.16.0.0/12`).
pub fn ip_matches_cidr(ip: IpAddr, cidr: &str) -> bool {
    let cidr = cidr.trim();
    if cidr == "*" {
        return true;
    }
    if let Ok(exact) = cidr.parse::<IpAddr>() {
        return ip == exact;
    }
    if let Some((cidr_ip, prefix_len)) = cidr.split_once('/') {
        if let (Ok(net_ip), Ok(prefix)) = (cidr_ip.parse::<IpAddr>(), prefix_len.parse::<u8>()) {
            match (ip, net_ip) {
                (IpAddr::V4(client), IpAddr::V4(net)) if prefix <= 32 => {
                    let mask = if prefix == 0 {
                        0
                    } else {
                        !0u32 << (32 - prefix)
                    };
                    if (u32::from(client) & mask) == (u32::from(net) & mask) {
                        return true;
                    }
                }
                (IpAddr::V6(client), IpAddr::V6(net)) if prefix <= 128 => {
                    let mask = if prefix == 0 {
                        0
                    } else {
                        !0u128 << (128 - prefix)
                    };
                    if (u128::from(client) & mask) == (u128::from(net) & mask) {
                        return true;
                    }
                }
                _ => {}
            }
        }
    }
    false
}

/// Windows `netsh advfirewall` backend for development and Windows servers.
#[derive(Debug, Default)]
pub struct WindowsNetshBackend {
    created_rules: Arc<std::sync::Mutex<HashSet<IpAddr>>>,
}

impl WindowsNetshBackend {
    pub fn new() -> Self {
        Self {
            created_rules: Arc::new(std::sync::Mutex::new(HashSet::new())),
        }
    }
}

impl FirewallBackend for WindowsNetshBackend {
    fn ban_ip(&self, ip: IpAddr, _duration: Duration) -> Result<(), FirewallError> {
        let rule_name = format!("Zenith_Block_{}", ip);
        let out = Command::new("netsh")
            .args([
                "advfirewall",
                "firewall",
                "add",
                "rule",
                &format!("name={}", rule_name),
                "dir=in",
                "action=block",
                &format!("remoteip={}", ip),
            ])
            .output()?;

        if out.status.success() {
            info!(ip = %ip, "Windows firewall rule created: {}", rule_name);
            if let Ok(mut set) = self.created_rules.lock() {
                set.insert(ip);
            }
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            Err(FirewallError::CommandFailed {
                code: out.status.code(),
                stderr,
            })
        }
    }

    fn unban_ip(&self, ip: IpAddr) -> Result<(), FirewallError> {
        let rule_name = format!("Zenith_Block_{}", ip);
        let out = Command::new("netsh")
            .args([
                "advfirewall",
                "firewall",
                "delete",
                "rule",
                &format!("name={}", rule_name),
            ])
            .output()?;

        if out.status.success() {
            if let Ok(mut set) = self.created_rules.lock() {
                set.remove(&ip);
            }
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            Err(FirewallError::CommandFailed {
                code: out.status.code(),
                stderr,
            })
        }
    }

    fn flush(&self) -> Result<(), FirewallError> {
        // Delete only rules specifically created by Zenith, never system-wide rules
        let ips: Vec<IpAddr> = if let Ok(set) = self.created_rules.lock() {
            set.iter().copied().collect()
        } else {
            Vec::new()
        };

        for ip in ips {
            let _ = self.unban_ip(ip);
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        "windows_netsh"
    }
}

/// Event sent to the background firewall synchronization worker.
#[derive(Debug, Clone)]
struct FirewallTask {
    ip: IpAddr,
    duration: Duration,
    reason: HardViolationReason,
}

/// Asynchronous coordinator managing OS firewall blocklists without stalling runtime reactors.
#[derive(Clone)]
pub struct FirewallManager {
    backend: Arc<dyn FirewallBackend>,
    whitelist: Arc<std::sync::RwLock<HashSet<IpAddr>>>,
    whitelist_cidrs: Arc<std::sync::RwLock<Vec<String>>>,
    dedup_cache: Arc<Mutex<HashMap<IpAddr, Instant>>>,
    task_tx: mpsc::UnboundedSender<FirewallTask>,
    enabled: bool,
    default_duration: Duration,
}

impl FirewallManager {
    /// Creates a new `FirewallManager` and spawns its asynchronous background processing loop.
    pub fn new(
        backend: Arc<dyn FirewallBackend>,
        enabled: bool,
        default_duration: Duration,
    ) -> Self {
        let mut initial_whitelist = HashSet::new();
        initial_whitelist.insert(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
        initial_whitelist.insert(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1)));

        let whitelist = Arc::new(std::sync::RwLock::new(initial_whitelist));
        let whitelist_cidrs = Arc::new(std::sync::RwLock::new(Vec::<String>::new()));
        let dedup_cache = Arc::new(Mutex::new(HashMap::new()));
        let (task_tx, mut task_rx) = mpsc::unbounded_channel::<FirewallTask>();

        let worker_backend = Arc::clone(&backend);
        let worker_whitelist = Arc::clone(&whitelist);
        let worker_cidrs = Arc::clone(&whitelist_cidrs);

        // Background worker ensuring zero game thread blocking
        tokio::spawn(async move {
            while let Some(task) = task_rx.recv().await {
                // Double check whitelist (exact IP and CIDR subnets)
                let is_immune = worker_whitelist.read().unwrap().contains(&task.ip)
                    || worker_cidrs
                        .read()
                        .unwrap()
                        .iter()
                        .any(|c| ip_matches_cidr(task.ip, c));

                if is_immune {
                    debug!(ip = %task.ip, "Firewall worker ignored whitelisted IP");
                    continue;
                }

                info!(
                    ip = %task.ip,
                    reason = %task.reason,
                    backend = worker_backend.name(),
                    duration_secs = task.duration.as_secs(),
                    "Escalating malicious host to Kernel-Level packet drop"
                );

                let backend_clone = Arc::clone(&worker_backend);
                let ip = task.ip;
                let duration = task.duration;

                // Offload synchronous command execution to blocking threadpool
                let res =
                    tokio::task::spawn_blocking(move || backend_clone.ban_ip(ip, duration)).await;

                match res {
                    Ok(Ok(())) => {
                        debug!(ip = %ip, "Kernel drop applied successfully");
                    }
                    Ok(Err(e)) => {
                        warn!(ip = %ip, error = %e, "Failed to apply OS firewall rule (continuing safely)");
                    }
                    Err(join_err) => {
                        error!(error = %join_err, "Firewall worker task panicked");
                    }
                }
            }
        });

        Self {
            backend,
            whitelist,
            whitelist_cidrs,
            dedup_cache,
            task_tx,
            enabled,
            default_duration,
        }
    }

    /// Adds an IP to the protected whitelist that can never be banned at the OS layer.
    pub fn add_whitelist(&self, ip: IpAddr) {
        self.whitelist.write().unwrap().insert(ip);
    }

    /// Adds a CIDR subnet block (e.g. `10.0.0.0/8`, `172.16.0.0/12`) to the protected whitelist.
    pub fn add_whitelist_cidr(&self, cidr: &str) {
        self.whitelist_cidrs.write().unwrap().push(cidr.to_string());
    }

    /// Checks whether an IP is immune from firewall jailing.
    #[must_use]
    pub fn is_whitelisted(&self, ip: &IpAddr) -> bool {
        if self.whitelist.read().unwrap().contains(ip) {
            return true;
        }

        if self
            .whitelist_cidrs
            .read()
            .unwrap()
            .iter()
            .any(|c| ip_matches_cidr(*ip, c))
        {
            return true;
        }

        // Loopback protection
        match ip {
            IpAddr::V4(v4) if v4.is_loopback() => true,
            IpAddr::V6(v6) if v6.is_loopback() => true,
            _ => false,
        }
    }

    /// Escalates an undisputed malicious actor to the kernel drop set.
    ///
    /// Deduplicates duplicate triggers within a 60-second window to prevent flooding OS APIs.
    pub async fn escalate(&self, ip: IpAddr, reason: HardViolationReason) {
        if !self.enabled {
            debug!(ip = %ip, reason = %reason, "Firewall escalation skipped (feature disabled)");
            return;
        }

        if self.is_whitelisted(&ip) {
            warn!(ip = %ip, reason = %reason, "Firewall escalation rejected: IP is whitelisted or loopback");
            return;
        }

        let now = Instant::now();
        let mut cache = self.dedup_cache.lock().await;

        // Prune entries older than 2 minutes
        cache.retain(|_, &mut exp| exp > now);

        if let Some(&expires) = cache.get(&ip) {
            if now < expires {
                debug!(ip = %ip, "Firewall escalation suppressed by dedup window");
                return;
            }
        }

        // Set dedup cooldown of 60 seconds
        cache.insert(ip, now + Duration::from_secs(60));
        drop(cache);

        let _ = self.task_tx.send(FirewallTask {
            ip,
            duration: self.default_duration,
            reason,
        });
    }

    /// Backend identifier name.
    #[must_use]
    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }

    /// Whether firewall synchronization is actively enabled.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_firewall_whitelist_protection() {
        let noop = Arc::new(NoopFirewall::new());
        let manager = FirewallManager::new(noop.clone(), true, Duration::from_secs(60));

        let loopback = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        manager
            .escalate(
                loopback,
                HardViolationReason::BlacklistedBrand {
                    brand: "mcstorm".into(),
                },
            )
            .await;

        // Allow background worker a moment to process
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(!noop.is_banned(&loopback));
        assert_eq!(noop.banned_count(), 0);
    }

    #[tokio::test]
    async fn test_firewall_deduplication() {
        let noop = Arc::new(NoopFirewall::new());
        let manager = FirewallManager::new(noop.clone(), true, Duration::from_secs(60));

        let attacker_ip = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 42));

        // Send 5 rapid escalations
        for _ in 0..5 {
            manager
                .escalate(
                    attacker_ip,
                    HardViolationReason::ExtremePacketFlooding { burst_count: 350 },
                )
                .await;
        }

        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(noop.is_banned(&attacker_ip));
        assert_eq!(noop.banned_count(), 1);
    }

    #[tokio::test]
    async fn test_firewall_disabled_mode() {
        let noop = Arc::new(NoopFirewall::new());
        let manager = FirewallManager::new(noop.clone(), false, Duration::from_secs(60));

        let attacker_ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 99));
        manager
            .escalate(
                attacker_ip,
                HardViolationReason::MalformedFrameOverflow {
                    phase: ProtocolPhase::Handshake,
                    size: 800,
                    max: 512,
                },
            )
            .await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(!noop.is_banned(&attacker_ip));
        assert_eq!(noop.banned_count(), 0);
    }

    #[test]
    fn test_hard_violation_display_formatting() {
        let reason = HardViolationReason::MalformedFrameOverflow {
            phase: ProtocolPhase::Status,
            size: 250,
            max: 128,
        };
        assert!(format!("{}", reason).contains("Oversized frame"));

        let brand_reason = HardViolationReason::BlacklistedBrand {
            brand: "mctool".into(),
        };
        assert!(format!("{}", brand_reason).contains("mctool"));
    }

    #[tokio::test]
    async fn test_firewall_cidr_whitelist() {
        let noop = Arc::new(NoopFirewall::new());
        let manager = FirewallManager::new(noop, true, Duration::from_secs(60));

        manager.add_whitelist_cidr("10.0.0.0/8");
        manager.add_whitelist_cidr("172.16.0.0/12");
        manager.add_whitelist_cidr("192.168.1.0/24");

        assert!(manager.is_whitelisted(&"10.50.2.1".parse().unwrap()));
        assert!(manager.is_whitelisted(&"172.20.10.5".parse().unwrap()));
        assert!(manager.is_whitelisted(&"192.168.1.100".parse().unwrap()));

        assert!(!manager.is_whitelisted(&"192.168.2.100".parse().unwrap()));
        assert!(!manager.is_whitelisted(&"8.8.8.8".parse().unwrap()));
    }
}
