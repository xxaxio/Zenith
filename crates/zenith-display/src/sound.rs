//! Protocol 777 SoundEffect packet encoders (Packet ID 0x62).

use bytes::{BufMut, BytesMut};
use zenith_wire::{encode_packet, write_string, write_varint};

/// Packet ID for Sound Effect in Protocol 777 Play state.
pub const PACKET_ID_SOUND_EFFECT: i32 = 119;

/// Sound category enumeration according to vanilla Protocol 777.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum SoundCategory {
    Master = 0,
    Music = 1,
    Records = 2,
    Weather = 3,
    Blocks = 4,
    Hostile = 5,
    Neutral = 6,
    Players = 7,
    Ambient = 8,
    Voice = 9,
}

/// Encodes a SoundEffect packet (`0x62`) playing a sound event at specific coordinates.
///
/// Coordinates are converted to fixed-point integers (`coord * 8.0`).
#[must_use]
pub fn encode_sound_packet(
    sound_id: &str,
    category: SoundCategory,
    x: f64,
    y: f64,
    z: f64,
    volume: f32,
    pitch: f32,
) -> Vec<u8> {
    let mut payload = BytesMut::with_capacity(64 + sound_id.len());

    // Sound Event ID: 0 represents a named custom sound identifier
    write_varint(0, &mut payload);
    write_string(sound_id, &mut payload);
    // Fixed range flag (Option<f32> - None represented as boolean false)
    payload.put_u8(0);

    // Sound category
    write_varint(category as i32, &mut payload);

    // Fixed-point coordinates (x * 8, y * 8, z * 8)
    payload.put_i32((x * 8.0) as i32);
    payload.put_i32((y * 8.0) as i32);
    payload.put_i32((z * 8.0) as i32);

    // Volume & Pitch
    payload.put_f32(volume);
    payload.put_f32(pitch);

    // Seed (randomizer for variations, default 0)
    payload.put_i64(0);

    encode_packet(PACKET_ID_SOUND_EFFECT, &payload)
}

/// Convenience helper to encode a UI click sound (`minecraft:ui.button.click`).
#[must_use]
pub fn encode_click_sound(pitch: f32) -> Vec<u8> {
    encode_sound_packet(
        "minecraft:ui.button.click",
        SoundCategory::Master,
        0.0,
        64.0,
        0.0,
        1.0,
        pitch,
    )
}

/// Convenience helper to encode a challenge completion sound (`minecraft:ui.toast.challenge_complete`).
#[must_use]
pub fn encode_transfer_sound(pitch: f32) -> Vec<u8> {
    encode_sound_packet(
        "minecraft:ui.toast.challenge_complete",
        SoundCategory::Master,
        0.0,
        64.0,
        0.0,
        1.0,
        pitch,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sound_packet_encoding() {
        let pkt = encode_sound_packet(
            "minecraft:ui.button.click",
            SoundCategory::Master,
            0.0,
            64.0,
            0.0,
            1.0,
            1.5,
        );
        assert!(!pkt.is_empty());
        assert!(pkt.len() > 20);
    }
}
