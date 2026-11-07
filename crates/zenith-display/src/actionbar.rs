//! Protocol 777 Set Actionbar Text and System Chat packet encoders.

use bytes::BytesMut;
use zenith_wire::{encode_packet, write_nbt_string};

/// Packet ID for Set Actionbar Text in Protocol 777 Play state.
pub const PACKET_ID_ACTIONBAR_TEXT: i32 = 89;

/// Packet ID for System Chat Message in Protocol 777 Play state.
pub const PACKET_ID_SYSTEM_CHAT: i32 = 124;

/// Encodes a Set Actionbar Text packet (`89` in 26.3 / Protocol 777).
#[must_use]
pub fn encode_actionbar_packet(text: &str) -> Vec<u8> {
    let mut payload = BytesMut::with_capacity(text.len() + 8);
    write_nbt_string(text, &mut payload);
    encode_packet(PACKET_ID_ACTIONBAR_TEXT, &payload)
}

/// Encodes a System Chat Message packet (`124` in 26.3 / Protocol 777).
///
/// `overlay` sets whether the message displays in the actionbar slot (true) or chat box (false).
#[must_use]
pub fn encode_system_chat_packet(text: &str, overlay: bool) -> Vec<u8> {
    let mut payload = BytesMut::with_capacity(text.len() + 8);
    write_nbt_string(text, &mut payload);
    payload.extend_from_slice(&[if overlay { 1 } else { 0 }]);
    encode_packet(PACKET_ID_SYSTEM_CHAT, &payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_actionbar_packet_encoding() {
        let pkt = encode_actionbar_packet("{\"text\":\"Position #1\"}");
        assert!(!pkt.is_empty());
    }

    #[test]
    fn test_system_chat_encoding() {
        let pkt = encode_system_chat_packet("{\"text\":\"Welcome to Zenith\"}", false);
        assert!(!pkt.is_empty());
    }
}
