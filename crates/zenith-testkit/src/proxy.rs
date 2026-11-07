//! Lightweight HTTP CONNECT proxy tunnel connector for distributed adversary simulation.

use std::path::Path;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::debug;

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("IO error connecting via proxy: {0}")]
    Io(#[from] std::io::Error),
    #[error("Proxy responded with invalid HTTP status: {0}")]
    InvalidStatus(String),
    #[error("Proxy handshake timeout")]
    Timeout,
    #[error("Invalid proxy address specification: {0}")]
    InvalidAddress(String),
}

/// Represents an external HTTP CONNECT proxy endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpProxy {
    pub host: String,
    pub port: u16,
}

impl HttpProxy {
    /// Parses an address string like `"http://1.2.3.4:8080"` or `"1.2.3.4:8080"`.
    pub fn parse(addr: &str) -> Result<Self, ProxyError> {
        let clean = addr.trim();
        let clean = clean.strip_prefix("http://").unwrap_or(clean);
        let clean = clean.strip_prefix("https://").unwrap_or(clean);

        let parts: Vec<&str> = clean.split(':').collect();
        if parts.len() != 2 {
            return Err(ProxyError::InvalidAddress(addr.to_string()));
        }

        let host = parts[0].to_string();
        let port: u16 = parts[1]
            .parse()
            .map_err(|_| ProxyError::InvalidAddress(addr.to_string()))?;

        Ok(Self { host, port })
    }

    /// Establishes an HTTP CONNECT tunnel through this proxy to the target address.
    pub async fn connect_tunnel(
        &self,
        target_host: &str,
        target_port: u16,
        timeout: std::time::Duration,
    ) -> Result<TcpStream, ProxyError> {
        let proxy_addr = format!("{}:{}", self.host, self.port);
        let connect_fut = TcpStream::connect(&proxy_addr);

        let mut stream = tokio::time::timeout(timeout, connect_fut)
            .await
            .map_err(|_| ProxyError::Timeout)?
            .map_err(ProxyError::Io)?;

        // Send HTTP CONNECT request
        let req = format!(
            "CONNECT {target_host}:{target_port} HTTP/1.1\r\nHost: {target_host}:{target_port}\r\nUser-Agent: Zenith-Testkit/1.0\r\n\r\n"
        );
        stream.write_all(req.as_bytes()).await?;

        // Read HTTP response header until "\r\n\r\n"
        let mut buf = [0u8; 1024];
        let mut total_read = 0;
        let mut status_line = String::new();

        loop {
            let n = stream.read(&mut buf[total_read..]).await?;
            if n == 0 {
                return Err(ProxyError::InvalidStatus(
                    "Proxy closed connection during handshake".to_string(),
                ));
            }
            total_read += n;

            let text = String::from_utf8_lossy(&buf[..total_read]);
            if text.contains("\r\n\r\n") {
                if let Some(first_line) = text.lines().next() {
                    status_line = first_line.to_string();
                }
                debug!(status = %status_line, "HTTP CONNECT tunnel established");
                break;
            }

            if total_read >= buf.len() {
                return Err(ProxyError::InvalidStatus(
                    "HTTP CONNECT response header too large".to_string(),
                ));
            }
        }

        if !status_line.contains("200") {
            return Err(ProxyError::InvalidStatus(status_line));
        }

        Ok(stream)
    }
}

/// Loads a pool of HTTP proxies from a text file (one proxy URL/IP:port per line).
/// Ignores empty lines and comments starting with `#`.
pub fn load_proxy_pool(path: &Path) -> Result<Vec<HttpProxy>, ProxyError> {
    let content = std::fs::read_to_string(path)?;
    let mut proxies = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Ok(proxy) = HttpProxy::parse(trimmed) {
            proxies.push(proxy);
        }
    }

    Ok(proxies)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_http_proxy() {
        let p1 = HttpProxy::parse("http://192.168.1.1:8080").unwrap();
        assert_eq!(p1.host, "192.168.1.1");
        assert_eq!(p1.port, 8080);

        let p2 = HttpProxy::parse("10.0.0.1:3128").unwrap();
        assert_eq!(p2.host, "10.0.0.1");
        assert_eq!(p2.port, 3128);
    }
}
