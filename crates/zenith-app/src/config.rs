//! Unified production configuration parser for Zenith (`zenith.toml`).

use serde::{Deserialize, Serialize};
use std::env;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default = "default_target")]
    pub target: String,
    #[serde(default = "default_mode")]
    pub mode: String,
}

fn default_bind() -> String {
    "0.0.0.0:25565".to_string()
}
fn default_target() -> String {
    "127.0.0.1:25566".to_string()
}
fn default_mode() -> String {
    "limbo".to_string()
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            target: default_target(),
            mode: default_mode(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueConfig {
    #[serde(default = "default_wave_size")]
    pub wave_size: usize,
    #[serde(default = "default_wave_interval")]
    pub wave_interval_ms: u64,
    #[serde(default = "default_sorting")]
    pub sorting: String,
    #[serde(default = "default_arrival_window")]
    pub arrival_window_ms: u64,
    #[serde(default = "default_groups_file")]
    pub groups_file: Option<PathBuf>,
}

fn default_wave_size() -> usize {
    5
}
fn default_wave_interval() -> u64 {
    4000
}
fn default_sorting() -> String {
    "fifo".to_string()
}
fn default_arrival_window() -> u64 {
    0
}
fn default_groups_file() -> Option<PathBuf> {
    Some(PathBuf::from("groups.toml"))
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            wave_size: default_wave_size(),
            wave_interval_ms: default_wave_interval(),
            sorting: default_sorting(),
            arrival_window_ms: default_arrival_window(),
            groups_file: default_groups_file(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotemSettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_probe_interval")]
    pub probe_interval_ms: u64,
    #[serde(default = "default_probe_timeout")]
    pub probe_timeout_ms: u64,
    #[serde(default = "default_two")]
    pub consecutive_failures: u32,
    #[serde(default = "default_two")]
    pub consecutive_successes: u32,
}

fn default_true() -> bool {
    true
}
fn default_probe_interval() -> u64 {
    1000
}
fn default_probe_timeout() -> u64 {
    600
}
fn default_two() -> u32 {
    2
}

impl Default for TotemSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            probe_interval_ms: default_probe_interval(),
            probe_timeout_ms: default_probe_timeout(),
            consecutive_failures: default_two(),
            consecutive_successes: default_two(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurboConfig {
    #[serde(default)]
    pub enabled: bool,
    pub endpoint: Option<String>,
    #[serde(default = "default_true")]
    pub pre_warm: bool,
}

impl Default for TurboConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: None,
            pre_warm: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FingerprintSettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub require_client_info: bool,
    #[serde(default = "default_min_view_distance")]
    pub min_view_distance: i8,
    #[serde(default)]
    pub require_brand: bool,
    #[serde(default = "default_blocked_brands")]
    pub blocked_brands: Vec<String>,
}

fn default_min_view_distance() -> i8 {
    1
}
fn default_blocked_brands() -> Vec<String> {
    vec![
        "mcbot".to_string(),
        "crafatar".to_string(),
        "botnet".to_string(),
        "headless".to_string(),
    ]
}

impl Default for FingerprintSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            require_client_info: true,
            min_view_distance: default_min_view_distance(),
            require_brand: false,
            blocked_brands: default_blocked_brands(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShieldSettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_max_conn_per_ip")]
    pub max_connections_per_ip: usize,
    #[serde(default = "default_ip_rate_limit")]
    pub ip_rate_limit: f64,
    #[serde(default = "default_ip_burst")]
    pub ip_burst_capacity: f64,
    #[serde(default = "default_subnet_rate_limit")]
    pub subnet_rate_limit: f64,
    #[serde(default = "default_subnet_burst")]
    pub subnet_burst_capacity: f64,
    #[serde(default = "default_five")]
    pub violation_threshold: u32,
    #[serde(default = "default_jail_duration")]
    pub jail_duration_secs: u64,
    #[serde(default = "default_true")]
    pub enable_challenge: bool,
    #[serde(default = "default_challenge_timeout")]
    pub challenge_timeout_ms: u64,
    #[serde(default)]
    pub fingerprint: FingerprintSettings,
    #[serde(default)]
    pub firewall: FirewallSettings,
}

/// Kernel-level packet filter and firewall synchronization settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirewallSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_firewall_backend")]
    pub backend: String,
    #[serde(default = "default_jail_duration")]
    pub jail_duration_secs: u64,
}

fn default_firewall_backend() -> String {
    "auto".to_string()
}

impl Default for FirewallSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: default_firewall_backend(),
            jail_duration_secs: default_jail_duration(),
        }
    }
}

fn default_max_conn_per_ip() -> usize {
    3
}
fn default_ip_rate_limit() -> f64 {
    2.0
}
fn default_ip_burst() -> f64 {
    5.0
}
fn default_subnet_rate_limit() -> f64 {
    10.0
}
fn default_subnet_burst() -> f64 {
    25.0
}
fn default_five() -> u32 {
    5
}
fn default_jail_duration() -> u64 {
    900 // 15 minutes default
}
fn default_challenge_timeout() -> u64 {
    3000
}

impl Default for ShieldSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            max_connections_per_ip: default_max_conn_per_ip(),
            ip_rate_limit: default_ip_rate_limit(),
            ip_burst_capacity: default_ip_burst(),
            subnet_rate_limit: default_subnet_rate_limit(),
            subnet_burst_capacity: default_subnet_burst(),
            violation_threshold: default_five(),
            jail_duration_secs: default_jail_duration(),
            enable_challenge: true,
            challenge_timeout_ms: default_challenge_timeout(),
            fingerprint: FingerprintSettings::default(),
            firewall: FirewallSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ForwardingConfig {
    pub hmac_secret: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_metrics_bind")]
    pub bind: String,
}

fn default_metrics_bind() -> String {
    "0.0.0.0:25580".to_string()
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bind: default_metrics_bind(),
        }
    }
}

/// Ingress reverse proxy configuration (PROXY Protocol v2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_trusted_proxies")]
    pub trusted_proxies: Vec<String>,
}

fn default_trusted_proxies() -> Vec<String> {
    vec![
        "127.0.0.1".to_string(),
        "::1".to_string(),
        "10.0.0.0/8".to_string(),
        "172.16.0.0/12".to_string(),
        "192.168.0.0/16".to_string(),
    ]
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            trusted_proxies: default_trusted_proxies(),
        }
    }
}

/// Automated lifecycle notifications configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BroadcastConfig {
    #[serde(default = "default_true")]
    pub notify_on_backend_down: bool,
    #[serde(default = "default_true")]
    pub notify_on_backend_up: bool,
}

impl Default for BroadcastConfig {
    fn default() -> Self {
        Self {
            notify_on_backend_down: true,
            notify_on_backend_up: true,
        }
    }
}

/// Root Zenith server configuration structure.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ZenithConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub queue: QueueConfig,
    #[serde(default)]
    pub totem: TotemSettings,
    #[serde(default)]
    pub turbo: TurboConfig,
    #[serde(default)]
    pub shield: ShieldSettings,
    #[serde(default)]
    pub forwarding: ForwardingConfig,
    #[serde(default)]
    pub metrics: MetricsConfig,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub broadcast: BroadcastConfig,
}

impl ZenithConfig {
    /// Loads configuration from file if present, with environment variable overrides.
    pub fn load_or_default(path_opt: Option<&Path>) -> Self {
        let mut config = if let Some(path) = path_opt {
            if path.exists() {
                match std::fs::read_to_string(path) {
                    Ok(content) => match toml::from_str::<ZenithConfig>(&content) {
                        Ok(cfg) => cfg,
                        Err(e) => {
                            tracing::warn!(error = %e, "Failed to parse config file, using defaults");
                            ZenithConfig::default()
                        }
                    },
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to read config file, using defaults");
                        ZenithConfig::default()
                    }
                }
            } else {
                ZenithConfig::default()
            }
        } else if Path::new("zenith.toml").exists() {
            std::fs::read_to_string("zenith.toml")
                .ok()
                .and_then(|c| toml::from_str(&c).ok())
                .unwrap_or_default()
        } else {
            ZenithConfig::default()
        };

        // Environment variable overrides
        if let Ok(b) = env::var("ZENITH_BIND") {
            config.server.bind = b;
        }
        if let Ok(t) = env::var("ZENITH_TARGET") {
            config.server.target = t;
        }
        if let Ok(m) = env::var("ZENITH_MODE") {
            config.server.mode = m;
        }
        if let Some(ws) = env::var("ZENITH_WAVE_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
        {
            config.queue.wave_size = ws;
        }
        if let Some(wi) = env::var("ZENITH_WAVE_INTERVAL_MS")
            .ok()
            .and_then(|s| s.parse().ok())
        {
            config.queue.wave_interval_ms = wi;
        }
        if let Ok(st) = env::var("ZENITH_QUEUE_SORTING") {
            config.queue.sorting = st;
        }
        if let Some(aw) = env::var("ZENITH_ARRIVAL_WINDOW_MS")
            .ok()
            .and_then(|s| s.parse().ok())
        {
            config.queue.arrival_window_ms = aw;
        }
        if let Ok(sec) = env::var("ZENITH_HMAC_SECRET") {
            config.forwarding.hmac_secret = Some(sec);
        }
        if let Ok(mb) = env::var("ZENITH_METRICS_BIND") {
            config.metrics.bind = mb;
        }
        if let Ok(proxy_val) = env::var("ZENITH_PROXY_ENABLED") {
            config.proxy.enabled = proxy_val == "1" || proxy_val.eq_ignore_ascii_case("true");
        }
        if let Ok(fw_en) = env::var("ZENITH_FIREWALL_ENABLED") {
            config.shield.firewall.enabled = fw_en == "1" || fw_en.eq_ignore_ascii_case("true");
        }
        if let Ok(fw_be) = env::var("ZENITH_FIREWALL_BACKEND") {
            config.shield.firewall.backend = fw_be;
        }

        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zenith_config_defaults_and_roundtrip() {
        let config = ZenithConfig::default();
        assert_eq!(config.server.bind, "0.0.0.0:25565");
        assert_eq!(config.server.target, "127.0.0.1:25566");
        assert_eq!(config.queue.sorting, "fifo");
        assert_eq!(config.queue.arrival_window_ms, 0);
        assert_eq!(config.shield.max_connections_per_ip, 3);
        assert_eq!(config.shield.jail_duration_secs, 900);
        assert!(!config.shield.firewall.enabled);
        assert_eq!(config.shield.firewall.backend, "auto");
        assert_eq!(config.metrics.bind, "0.0.0.0:25580");
        assert!(!config.proxy.enabled);
        assert!(config.broadcast.notify_on_backend_down);

        let toml_str = toml::to_string_pretty(&config).unwrap();
        assert!(toml_str.contains("jail_duration_secs = 900"));
        let parsed: ZenithConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.server.bind, "0.0.0.0:25565");
        assert_eq!(parsed.proxy.trusted_proxies.len(), 5);
    }
}
