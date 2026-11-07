//! HAProxy PROXY Protocol v2 binary decoder and parser.
//!
//! Enables Zenith to securely identify real player IP addresses when deployed
//! behind upstream L4/L7 reverse proxies, DDoS mitigation networks, or load balancers
//! (such as Cloudflare Spectrum, HAProxy, AWS NLB, or OVH Anti-DDoS).
//!
//! Reference: HAProxy PROXY protocol spec version 2.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use thiserror::Error;

/// The 12-byte magic header prefix for PROXY protocol version 2.
pub const PROXY_V2_MAGIC: [u8; 12] = [
    0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A,
];

/// Minimum required header size to inspect magic and payload length.
pub const PROXY_V2_MIN_HEADER_LEN: usize = 16;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProxyError {
    #[error("Not a PROXY protocol v2 header (magic signature mismatch)")]
    NotProxyV2,
    #[error("Unsupported PROXY protocol version: {0} (expected version 2)")]
    UnsupportedVersion(u8),
    #[error("Invalid PROXY command: {0}")]
    InvalidCommand(u8),
    #[error("Buffer underflow while reading PROXY protocol header")]
    BufferUnderflow,
    #[error("Invalid address length or format in PROXY payload")]
    InvalidAddress,
    #[error("IO error reading PROXY header: {0}")]
    Io(String),
}

/// Command byte indicator in PROXY v2 header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyCommand {
    /// LOCAL: Connection initiated by the proxy itself (e.g. health check).
    Local = 0x00,
    /// PROXY: Connection forwarded on behalf of an original client.
    Proxy = 0x01,
}

/// Decoded PROXY protocol v2 header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyHeader {
    pub command: ProxyCommand,
    pub src_addr: Option<SocketAddr>,
    pub dst_addr: Option<SocketAddr>,
}

impl ProxyHeader {
    /// Parses a PROXY protocol v2 header from a byte buffer.
    ///
    /// Returns:
    /// - `Ok(Some((header, bytes_consumed)))` if a complete valid PROXY v2 header is parsed.
    /// - `Ok(None)` if the buffer does not match the PROXY v2 magic signature (e.g. normal Minecraft handshake).
    /// - `Err(ProxyError::BufferUnderflow)` if buffer matches the prefix but is incomplete.
    /// - `Err(ProxyError)` on invalid version or malformed protocol fields.
    pub fn parse(buf: &[u8]) -> Result<Option<(Self, usize)>, ProxyError> {
        let check_len = buf.len().min(PROXY_V2_MAGIC.len());
        if check_len == 0 {
            return Err(ProxyError::BufferUnderflow);
        }

        // If the bytes we have do not match the magic prefix, it is definitely not PROXY v2
        if buf[..check_len] != PROXY_V2_MAGIC[..check_len] {
            return Ok(None);
        }

        if buf.len() < PROXY_V2_MIN_HEADER_LEN {
            return Err(ProxyError::BufferUnderflow);
        }

        let ver_cmd = buf[12];
        let version = ver_cmd >> 4;
        if version != 2 {
            return Err(ProxyError::UnsupportedVersion(version));
        }

        let command = match ver_cmd & 0x0F {
            0x00 => ProxyCommand::Local,
            0x01 => ProxyCommand::Proxy,
            cmd => return Err(ProxyError::InvalidCommand(cmd)),
        };

        let fam_proto = buf[13];
        let payload_len = u16::from_be_bytes([buf[14], buf[15]]) as usize;
        let total_len = PROXY_V2_MIN_HEADER_LEN + payload_len;

        if buf.len() < total_len {
            return Err(ProxyError::BufferUnderflow);
        }

        let payload = &buf[PROXY_V2_MIN_HEADER_LEN..total_len];

        let (src_addr, dst_addr) = match fam_proto {
            // AF_INET / STREAM (IPv4 TCP)
            0x11 => {
                if payload.len() < 12 {
                    return Err(ProxyError::InvalidAddress);
                }
                let src_ip = Ipv4Addr::new(payload[0], payload[1], payload[2], payload[3]);
                let dst_ip = Ipv4Addr::new(payload[4], payload[5], payload[6], payload[7]);
                let src_port = u16::from_be_bytes([payload[8], payload[9]]);
                let dst_port = u16::from_be_bytes([payload[10], payload[11]]);

                (
                    Some(SocketAddr::V4(SocketAddrV4::new(src_ip, src_port))),
                    Some(SocketAddr::V4(SocketAddrV4::new(dst_ip, dst_port))),
                )
            }
            // AF_INET6 / STREAM (IPv6 TCP)
            0x21 => {
                if payload.len() < 36 {
                    return Err(ProxyError::InvalidAddress);
                }
                let mut src_bytes = [0u8; 16];
                src_bytes.copy_from_slice(&payload[0..16]);
                let mut dst_bytes = [0u8; 16];
                dst_bytes.copy_from_slice(&payload[16..32]);

                let src_ip = Ipv6Addr::from(src_bytes);
                let dst_ip = Ipv6Addr::from(dst_bytes);
                let src_port = u16::from_be_bytes([payload[32], payload[33]]);
                let dst_port = u16::from_be_bytes([payload[34], payload[35]]);

                (
                    Some(SocketAddr::V6(SocketAddrV6::new(src_ip, src_port, 0, 0))),
                    Some(SocketAddr::V6(SocketAddrV6::new(dst_ip, dst_port, 0, 0))),
                )
            }
            // AF_UNSPEC or non-TCP (e.g. health checks or local probes)
            _ => (None, None),
        };

        Ok(Some((
            Self {
                command,
                src_addr,
                dst_addr,
            },
            total_len,
        )))
    }

    /// Helper to encode a synthetic PROXY v2 header for IPv4 TCP connections.
    pub fn encode_ipv4(src: SocketAddrV4, dst: SocketAddrV4) -> Vec<u8> {
        let mut out = Vec::with_capacity(PROXY_V2_MIN_HEADER_LEN + 12);
        out.extend_from_slice(&PROXY_V2_MAGIC);
        out.push(0x21); // Version 2 | Command 1 (Proxy)
        out.push(0x11); // AF_INET (1) | STREAM (1)
        out.extend_from_slice(&12u16.to_be_bytes()); // Length 12
        out.extend_from_slice(&src.ip().octets());
        out.extend_from_slice(&dst.ip().octets());
        out.extend_from_slice(&src.port().to_be_bytes());
        out.extend_from_slice(&dst.port().to_be_bytes());
        out
    }

    /// Helper to encode a synthetic PROXY v2 header for IPv6 TCP connections.
    pub fn encode_ipv6(src: SocketAddrV6, dst: SocketAddrV6) -> Vec<u8> {
        let mut out = Vec::with_capacity(PROXY_V2_MIN_HEADER_LEN + 36);
        out.extend_from_slice(&PROXY_V2_MAGIC);
        out.push(0x21); // Version 2 | Command 1 (Proxy)
        out.push(0x21); // AF_INET6 (2) | STREAM (1)
        out.extend_from_slice(&36u16.to_be_bytes()); // Length 36
        out.extend_from_slice(&src.ip().octets());
        out.extend_from_slice(&dst.ip().octets());
        out.extend_from_slice(&src.port().to_be_bytes());
        out.extend_from_slice(&dst.port().to_be_bytes());
        out
    }

    /// Helper to encode a LOCAL command header (e.g. proxy health probe).
    pub fn encode_local() -> Vec<u8> {
        let mut out = Vec::with_capacity(PROXY_V2_MIN_HEADER_LEN);
        out.extend_from_slice(&PROXY_V2_MAGIC);
        out.push(0x20); // Version 2 | Command 0 (Local)
        out.push(0x00); // AF_UNSPEC (0) | UNSPEC (0)
        out.extend_from_slice(&0u16.to_be_bytes()); // Length 0
        out
    }
}

/// Checks whether an incoming IP belongs to a trusted proxy list.
///
/// Supports exact IP matches (e.g. `127.0.0.1`, `::1`) and standard CIDR blocks
/// (e.g. `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`).
pub fn is_trusted_proxy(ip: IpAddr, trusted_list: &[String]) -> bool {
    if trusted_list.is_empty() {
        return false;
    }

    for item in trusted_list {
        let item = item.trim();
        if item == "*" {
            return true;
        }

        if let Ok(target_ip) = item.parse::<IpAddr>() {
            if ip == target_ip {
                return true;
            }
        } else if let Some((cidr_ip, prefix_len)) = item.split_once('/') {
            if let (Ok(net_ip), Ok(prefix)) = (cidr_ip.parse::<IpAddr>(), prefix_len.parse::<u8>())
            {
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
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proxy_v2_ipv4_parse_roundtrip() {
        let src: SocketAddrV4 = "198.51.100.22:54321".parse().unwrap();
        let dst: SocketAddrV4 = "203.0.113.1:25565".parse().unwrap();

        let encoded = ProxyHeader::encode_ipv4(src, dst);
        let parsed = ProxyHeader::parse(&encoded).unwrap();

        assert!(parsed.is_some());
        let (header, consumed) = parsed.unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(header.command, ProxyCommand::Proxy);
        assert_eq!(header.src_addr, Some(SocketAddr::V4(src)));
        assert_eq!(header.dst_addr, Some(SocketAddr::V4(dst)));
    }

    #[test]
    fn test_proxy_v2_ipv6_parse_roundtrip() {
        let src: SocketAddrV6 = "[2001:db8::1]:49152".parse().unwrap();
        let dst: SocketAddrV6 = "[2001:db8::2]:25565".parse().unwrap();

        let encoded = ProxyHeader::encode_ipv6(src, dst);
        let parsed = ProxyHeader::parse(&encoded).unwrap();

        assert!(parsed.is_some());
        let (header, consumed) = parsed.unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(header.command, ProxyCommand::Proxy);
        assert_eq!(header.src_addr, Some(SocketAddr::V6(src)));
        assert_eq!(header.dst_addr, Some(SocketAddr::V6(dst)));
    }

    #[test]
    fn test_proxy_v2_local_command() {
        let encoded = ProxyHeader::encode_local();
        let parsed = ProxyHeader::parse(&encoded).unwrap();

        assert!(parsed.is_some());
        let (header, consumed) = parsed.unwrap();
        assert_eq!(consumed, PROXY_V2_MIN_HEADER_LEN);
        assert_eq!(header.command, ProxyCommand::Local);
        assert_eq!(header.src_addr, None);
    }

    #[test]
    fn test_not_proxy_header_returns_none() {
        // Normal Minecraft Handshake packet begins with VarInt length (e.g. 0x0F, 0x00, ...)
        let mc_packet = [0x0F, 0x00, 0x09, 0x09, 0x37, 0x37, 0x37];
        let parsed = ProxyHeader::parse(&mc_packet).unwrap();
        assert!(parsed.is_none());
    }

    #[test]
    fn test_proxy_header_underflow() {
        let short = &PROXY_V2_MAGIC[..8];
        assert_eq!(ProxyHeader::parse(short), Err(ProxyError::BufferUnderflow));
    }

    #[test]
    fn test_trusted_proxy_cidr_evaluation() {
        let trusted = vec![
            "127.0.0.1".to_string(),
            "10.0.0.0/8".to_string(),
            "192.168.1.0/24".to_string(),
            "2001:db8::/32".to_string(),
        ];

        assert!(is_trusted_proxy("127.0.0.1".parse().unwrap(), &trusted));
        assert!(is_trusted_proxy("10.50.1.2".parse().unwrap(), &trusted));
        assert!(is_trusted_proxy("192.168.1.42".parse().unwrap(), &trusted));
        assert!(is_trusted_proxy(
            "2001:db8:ffff::1".parse().unwrap(),
            &trusted
        ));

        assert!(!is_trusted_proxy("192.168.2.1".parse().unwrap(), &trusted));
        assert!(!is_trusted_proxy("172.16.0.1".parse().unwrap(), &trusted));
        assert!(!is_trusted_proxy("8.8.8.8".parse().unwrap(), &trusted));
    }
}
