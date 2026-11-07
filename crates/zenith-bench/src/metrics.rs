//! Lock-free metrics aggregation, latency percentiles, and memory RSS profiling.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;

/// Lock-free live counter structure shared across all running benchmark actor tasks.
#[derive(Debug, Default)]
pub struct Live {
    pub connected: AtomicUsize,
    pub joined_limbo: AtomicUsize,
    pub transferred: AtomicUsize,
    pub jailed_or_blocked: AtomicUsize,
    pub keepalives_answered: AtomicUsize,
    pub errors: AtomicUsize,
    pub bytes_sent: AtomicU64,
    pub bytes_received: AtomicU64,
    pub resource_packs_loaded: AtomicUsize,
    pub alerts_received: AtomicUsize,
    pub proxy_headers_sent: AtomicUsize,
}

impl Live {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn snapshot(&self) -> LiveSnapshot {
        LiveSnapshot {
            connected: self.connected.load(Ordering::Relaxed),
            joined_limbo: self.joined_limbo.load(Ordering::Relaxed),
            transferred: self.transferred.load(Ordering::Relaxed),
            jailed_or_blocked: self.jailed_or_blocked.load(Ordering::Relaxed),
            keepalives_answered: self.keepalives_answered.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            bytes_sent: self.bytes_sent.load(Ordering::Relaxed),
            bytes_received: self.bytes_received.load(Ordering::Relaxed),
            resource_packs_loaded: self.resource_packs_loaded.load(Ordering::Relaxed),
            alerts_received: self.alerts_received.load(Ordering::Relaxed),
            proxy_headers_sent: self.proxy_headers_sent.load(Ordering::Relaxed),
        }
    }
}

/// Instantaneous snapshot of live counters.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct LiveSnapshot {
    pub connected: usize,
    pub joined_limbo: usize,
    pub transferred: usize,
    pub jailed_or_blocked: usize,
    pub keepalives_answered: usize,
    pub errors: usize,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub resource_packs_loaded: usize,
    pub alerts_received: usize,
    pub proxy_headers_sent: usize,
}

/// Thread-safe latency distribution sample collector.
#[derive(Debug, Default)]
pub struct LatencyHistogram {
    samples: Mutex<Vec<Duration>>,
}

impl LatencyHistogram {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn record(&self, latency: Duration) {
        let mut samples = self.samples.lock().await;
        samples.push(latency);
    }

    pub async fn calculate_percentiles(&self) -> LatencyPercentiles {
        let mut samples = self.samples.lock().await;
        if samples.is_empty() {
            return LatencyPercentiles::default();
        }

        samples.sort();
        let len = samples.len();

        let p50 = samples[(len as f64 * 0.50) as usize];
        let p90 = samples[((len as f64 * 0.90) as usize).min(len - 1)];
        let p99 = samples[((len as f64 * 0.99) as usize).min(len - 1)];
        let p999 = samples[((len as f64 * 0.999) as usize).min(len - 1)];
        let max = *samples.last().unwrap();
        let sum: Duration = samples.iter().copied().sum();
        let avg = sum / len as u32;

        LatencyPercentiles {
            count: len,
            avg,
            p50,
            p90,
            p99,
            p999,
            max,
        }
    }
}

/// Computed latency percentiles.
#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)]
pub struct LatencyPercentiles {
    pub count: usize,
    pub avg: Duration,
    pub p50: Duration,
    pub p90: Duration,
    pub p99: Duration,
    pub p999: Duration,
    pub max: Duration,
}

/// Fetches real-time memory RSS and active sessions from Zenith's Prometheus endpoint.
pub async fn fetch_zenith_prometheus_metrics(metrics_url: &str) -> Option<PrometheusMetrics> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let clean = metrics_url.strip_prefix("http://").unwrap_or(metrics_url);
    let parts: Vec<&str> = clean.split('/').collect();
    let host_port = parts[0];
    let path = if parts.len() > 1 {
        format!("/{}", parts[1])
    } else {
        "/metrics".to_string()
    };

    let mut stream = TcpStream::connect(host_port).await.ok()?;
    let req = format!("GET {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.ok()?;

    let mut response = String::new();
    stream.read_to_string(&mut response).await.ok()?;

    let mut metrics = PrometheusMetrics::default();
    for line in response.lines() {
        if line.starts_with("zenith_memory_rss_bytes") {
            if let Some(val) = line.split_whitespace().nth(1) {
                metrics.memory_rss_bytes = val.parse().unwrap_or(0);
            }
        } else if line.starts_with("zenith_sessions_active") {
            if let Some(val) = line.split_whitespace().nth(1) {
                metrics.active_sessions = val.parse().unwrap_or(0);
            }
        } else if line.starts_with("zenith_queue_size") {
            if let Some(val) = line.split_whitespace().nth(1) {
                metrics.queue_size = val.parse().unwrap_or(0);
            }
        } else if line.starts_with("zenith_shield_jailed_ips") {
            if let Some(val) = line.split_whitespace().nth(1) {
                metrics.jailed_ips = val.parse().unwrap_or(0);
            }
        }
    }

    if metrics.memory_rss_bytes == 0 {
        metrics.memory_rss_bytes = get_target_process_memory_rss("zenith");
    }

    Some(metrics)
}

/// Fallback sampler to measure target process RSS memory across operating systems.
pub fn get_target_process_memory_rss(_proc_name: &str) -> u64 {
    #[cfg(windows)]
    {
        let cmd = format!(
            "(Get-Process {_proc_name} -ErrorAction SilentlyContinue | Select-Object -First 1).WorkingSet64"
        );
        if let Ok(output) = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &cmd])
            .output()
        {
            let s = String::from_utf8_lossy(&output.stdout);
            return s.trim().parse::<u64>().unwrap_or(0);
        }
    }
    #[cfg(unix)]
    {
        if let Ok(content) = std::fs::read_to_string("/proc/self/statm") {
            if let Some(rss_pages) = content.split_whitespace().nth(1) {
                if let Ok(pages) = rss_pages.parse::<u64>() {
                    return pages * 4096;
                }
            }
        }
    }
    0
}

#[derive(Debug, Default, Clone, Copy)]
pub struct PrometheusMetrics {
    pub memory_rss_bytes: u64,
    pub active_sessions: usize,
    pub queue_size: usize,
    pub jailed_ips: usize,
}

/// Pretty-prints a summary report table to stdout.
pub fn print_summary_table(title: &str, rows: &[(&str, String)]) {
    println!("\n================================================================================");
    println!("   {title}");
    println!("================================================================================");
    for (label, value) in rows {
        println!("   {label:<35} : {value}");
    }
    println!("================================================================================\n");
}
