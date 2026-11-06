//! Protocol 777 Clientbound Transfer packet encoder (Packet ID 0x7C).

use bytes::BytesMut;
use zenith_wire::{encode_packet, write_string, write_varint};

/// Packet ID for Clientbound Transfer in Protocol 777 Play state.
pub const PACKET_ID_TRANSFER: i32 = 0x7C;

/// Encodes a Clientbound Transfer packet (`0x7C`).
///
/// Instructs the Minecraft client to smoothly switch connections to `host:port`
/// without showing a disconnect screen or terminating the process.
#[must_use]
pub fn encode_transfer_packet(host: &str, port: u16) -> Vec<u8> {
    let mut payload = BytesMut::with_capacity(host.len() + 8);
    write_string(host, &mut payload);
    write_varint(port as i32, &mut payload);

    encode_packet(PACKET_ID_TRANSFER, &payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transfer_packet_encoding() {
        let pkt = encode_transfer_packet("play.horizon.dev", 25565);
        assert!(!pkt.is_empty());
        // Verify packet contains host string bytes
        let host_bytes = b"play.horizon.dev";
        assert!(pkt.windows(host_bytes.len()).any(|w| w == host_bytes));
    }
}
