//! Prometheus telemetry exporter (`GET /metrics`) for Zenith.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::{error, info};

/// Atomic operational metrics registry scraped by Prometheus.
#[derive(Debug, Default)]
pub struct ZenithMetrics {
    pub active_sessions: AtomicUsize,
    pub queue_size: AtomicUsize,
    pub transfers_total: AtomicU64,
    pub totem_backend_online: AtomicBool,
    pub totem_latency_ms: AtomicU32,
    pub turbo_mspt_us: AtomicU64,
    pub shield_dropped_frames_total: AtomicU64,
    pub shield_rate_limited_total: AtomicU64,
    pub shield_jailed_ips: AtomicUsize,
    pub shield_challenges_failed_total: AtomicU64,
}

impl ZenithMetrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Renders all metrics in standard Prometheus text exposition format (version 0.0.4).
    pub fn render_prometheus(&self) -> String {
        let mut out = String::with_capacity(2048);

        out.push_str(
            "# HELP zenith_sessions_active Current number of players in virtual dimension\n",
        );
        out.push_str("# TYPE zenith_sessions_active gauge\n");
        out.push_str(&format!(
            "zenith_sessions_active {}\n\n",
            self.active_sessions.load(Ordering::Relaxed)
        ));

        out.push_str(
            "# HELP zenith_queue_size Current number of players waiting in transit queue\n",
        );
        out.push_str("# TYPE zenith_queue_size gauge\n");
        out.push_str(&format!(
            "zenith_queue_size {}\n\n",
            self.queue_size.load(Ordering::Relaxed)
        ));

        out.push_str(
            "# HELP zenith_transfers_total Total number of players transferred to backend\n",
        );
        out.push_str("# TYPE zenith_transfers_total counter\n");
        out.push_str(&format!(
            "zenith_transfers_total {}\n\n",
            self.transfers_total.load(Ordering::Relaxed)
        ));

        out.push_str(
            "# HELP zenith_totem_backend_online Backend availability (1 = healthy, 0 = down)\n",
        );
        out.push_str("# TYPE zenith_totem_backend_online gauge\n");
        out.push_str(&format!(
            "zenith_totem_backend_online {}\n\n",
            if self.totem_backend_online.load(Ordering::Relaxed) {
                1
            } else {
                0
            }
        ));

        out.push_str("# HELP zenith_totem_latency_ms Backend round-trip latency in milliseconds\n");
        out.push_str("# TYPE zenith_totem_latency_ms gauge\n");
        out.push_str(&format!(
            "zenith_totem_latency_ms {}\n\n",
            self.totem_latency_ms.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP zenith_turbo_mspt_us Horizon MSPT in microseconds (from ctl.sock)\n");
        out.push_str("# TYPE zenith_turbo_mspt_us gauge\n");
        out.push_str(&format!(
            "zenith_turbo_mspt_us {}\n\n",
            self.turbo_mspt_us.load(Ordering::Relaxed)
        ));

        out.push_str(
            "# HELP zenith_shield_dropped_frames_total Malformed or oversized frames dropped\n",
        );
        out.push_str("# TYPE zenith_shield_dropped_frames_total counter\n");
        out.push_str(&format!(
            "zenith_shield_dropped_frames_total {}\n\n",
            self.shield_dropped_frames_total.load(Ordering::Relaxed)
        ));

        out.push_str(
            "# HELP zenith_shield_rate_limited_total Connections rejected by rate limiting\n",
        );
        out.push_str("# TYPE zenith_shield_rate_limited_total counter\n");
        out.push_str(&format!(
            "zenith_shield_rate_limited_total {}\n\n",
            self.shield_rate_limited_total.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP zenith_shield_jailed_ips Current number of IP addresses in jail\n");
        out.push_str("# TYPE zenith_shield_jailed_ips gauge\n");
        out.push_str(&format!(
            "zenith_shield_jailed_ips {}\n\n",
            self.shield_jailed_ips.load(Ordering::Relaxed)
        ));

        out.push_str(
            "# HELP zenith_shield_challenges_failed_total Total failed protocol challenges\n",
        );
        out.push_str("# TYPE zenith_shield_challenges_failed_total counter\n");
        out.push_str(&format!(
            "zenith_shield_challenges_failed_total {}\n",
            self.shield_challenges_failed_total.load(Ordering::Relaxed)
        ));

        out
    }
}

/// Spawns the lightweight asynchronous Prometheus HTTP metrics server.
pub async fn start_metrics_server(
    bind_addr: String,
    metrics: Arc<ZenithMetrics>,
) -> std::io::Result<()> {
    let listener = TcpListener::bind(&bind_addr).await?;
    info!(bind = %bind_addr, "Prometheus metrics exporter online at http://{}/metrics", bind_addr);

    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((mut socket, _)) => {
                    let metrics = Arc::clone(&metrics);
                    tokio::spawn(async move {
                        let mut buf = [0u8; 1024];
                        if let Ok(n) = socket.read(&mut buf).await {
                            let request = String::from_utf8_lossy(&buf[..n]);
                            if request.starts_with("GET /metrics") {
                                let body = metrics.render_prometheus();
                                let resp = format!(
                                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                    body.len(),
                                    body
                                );
                                let _ = socket.write_all(resp.as_bytes()).await;
                            } else {
                                let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                                let _ = socket.write_all(resp.as_bytes()).await;
                            }
                        }
                    });
                }
                Err(e) => {
                    error!(error = %e, "Metrics listener accept error");
                    break;
                }
            }
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prometheus_rendering_format() {
        let metrics = ZenithMetrics::new();
        metrics.active_sessions.store(100, Ordering::Relaxed);
        metrics.queue_size.store(85, Ordering::Relaxed);
        metrics.transfers_total.store(42, Ordering::Relaxed);
        metrics.totem_backend_online.store(true, Ordering::Relaxed);
        metrics.totem_latency_ms.store(15, Ordering::Relaxed);

        let rendered = metrics.render_prometheus();
        assert!(rendered.contains("zenith_sessions_active 100"));
        assert!(rendered.contains("zenith_queue_size 85"));
        assert!(rendered.contains("zenith_transfers_total 42"));
        assert!(rendered.contains("zenith_totem_backend_online 1"));
        assert!(rendered.contains("zenith_totem_latency_ms 15"));
    }

    #[tokio::test]
    async fn test_metrics_http_endpoint() {
        let metrics = ZenithMetrics::new();
        metrics.active_sessions.store(7, Ordering::Relaxed);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let bind_addr = format!("127.0.0.1:{port}");
        start_metrics_server(bind_addr.clone(), Arc::clone(&metrics))
            .await
            .unwrap();

        // Connect as HTTP client
        let mut client = tokio::net::TcpStream::connect(&bind_addr).await.unwrap();
        client
            .write_all(b"GET /metrics HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();

        let mut resp = String::new();
        let mut buf = vec![0u8; 4096];
        let n = client.read(&mut buf).await.unwrap();
        resp.push_str(&String::from_utf8_lossy(&buf[..n]));

        assert!(resp.starts_with("HTTP/1.1 200 OK"));
        assert!(resp.contains("zenith_sessions_active 7"));
    }
}
