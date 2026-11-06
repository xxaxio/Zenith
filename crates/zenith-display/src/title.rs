//! Protocol 777 Title, Subtitle, and Title Times packet encoders.

use bytes::{BufMut, BytesMut};
use zenith_wire::{encode_packet, write_string};

/// Packet ID for Set Title Text in Protocol 777 Play state.
pub const PACKET_ID_SET_TITLE: i32 = 0x67;

/// Packet ID for Set Subtitle Text in Protocol 777 Play state.
pub const PACKET_ID_SET_SUBTITLE: i32 = 0x69;

/// Packet ID for Set Title Animation Times in Protocol 777 Play state.
pub const PACKET_ID_SET_TITLE_TIMES: i32 = 0x6A;

/// Packet ID for Clear Titles in Protocol 777 Play state.
pub const PACKET_ID_CLEAR_TITLES: i32 = 0x0D;

/// Encodes a Set Title Text packet (`0x67`).
#[must_use]
pub fn encode_title_packet(title_json: &str) -> Vec<u8> {
    let mut payload = BytesMut::with_capacity(title_json.len() + 8);
    write_string(title_json, &mut payload);
    encode_packet(PACKET_ID_SET_TITLE, &payload)
}

/// Encodes a Set Subtitle Text packet (`0x69`).
#[must_use]
pub fn encode_subtitle_packet(subtitle_json: &str) -> Vec<u8> {
    let mut payload = BytesMut::with_capacity(subtitle_json.len() + 8);
    write_string(subtitle_json, &mut payload);
    encode_packet(PACKET_ID_SET_SUBTITLE, &payload)
}

/// Encodes a Set Title Animation Times packet (`0x6A`).
///
/// Durations are specified in game ticks (20 ticks = 1 second).
#[must_use]
pub fn encode_title_times_packet(
    fade_in_ticks: i32,
    stay_ticks: i32,
    fade_out_ticks: i32,
) -> Vec<u8> {
    let mut payload = BytesMut::with_capacity(12);
    payload.put_i32(fade_in_ticks);
    payload.put_i32(stay_ticks);
    payload.put_i32(fade_out_ticks);
    encode_packet(PACKET_ID_SET_TITLE_TIMES, &payload)
}

/// Encodes a Clear Titles packet (`0x0D`).
#[must_use]
pub fn encode_clear_titles_packet(reset_times: bool) -> Vec<u8> {
    let mut payload = BytesMut::with_capacity(1);
    payload.put_u8(if reset_times { 1 } else { 0 });
    encode_packet(PACKET_ID_CLEAR_TITLES, &payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_title_packet_encodings() {
        let title_pkt = encode_title_packet("{\"text\":\"QUEUE OPEN\"}");
        assert!(!title_pkt.is_empty());

        let sub_pkt = encode_subtitle_packet("{\"text\":\"Moving forward\"}");
        assert!(!sub_pkt.is_empty());

        let times_pkt = encode_title_times_packet(10, 60, 20);
        assert!(!times_pkt.is_empty());

        let clear_pkt = encode_clear_titles_packet(true);
        assert!(!clear_pkt.is_empty());
    }
}
