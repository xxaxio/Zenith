//! Protocol 777 BossEvent packet encoders (Packet ID 0x0A).

use bytes::{BufMut, BytesMut};
use zenith_wire::{encode_packet, write_nbt_string, write_varint};

use crate::config::{BossBarColor, BossBarDivision};

/// Packet ID for Clientbound BossEvent in Protocol 777 Play state.
pub const PACKET_ID_BOSS_EVENT: i32 = 9;

/// Encodes an Action 0 (`Add`) BossBar packet.
#[must_use]
pub fn encode_bossbar_add(
    uuid: u128,
    title: &str,
    health: f32,
    color: BossBarColor,
    division: BossBarDivision,
) -> Vec<u8> {
    let mut payload = BytesMut::with_capacity(64 + title.len());
    payload.put_u128(uuid);
    write_varint(0, &mut payload); // Action 0: Add
    write_nbt_string(title, &mut payload);
    payload.put_f32(health.clamp(0.0, 1.0));
    write_varint(color as i32, &mut payload);
    write_varint(division as i32, &mut payload);
    payload.put_u8(0); // Flags: none

    encode_packet(PACKET_ID_BOSS_EVENT, &payload)
}

/// Encodes an Action 1 (`Remove`) BossBar packet.
#[must_use]
pub fn encode_bossbar_remove(uuid: u128) -> Vec<u8> {
    let mut payload = BytesMut::with_capacity(20);
    payload.put_u128(uuid);
    write_varint(1, &mut payload); // Action 1: Remove

    encode_packet(PACKET_ID_BOSS_EVENT, &payload)
}

/// Encodes an Action 2 (`Update Health`) BossBar packet.
#[must_use]
pub fn encode_bossbar_health(uuid: u128, health: f32) -> Vec<u8> {
    let mut payload = BytesMut::with_capacity(24);
    payload.put_u128(uuid);
    write_varint(2, &mut payload); // Action 2: Update Health
    payload.put_f32(health.clamp(0.0, 1.0));

    encode_packet(PACKET_ID_BOSS_EVENT, &payload)
}

/// Encodes an Action 3 (`Update Title`) BossBar packet.
#[must_use]
pub fn encode_bossbar_title(uuid: u128, title: &str) -> Vec<u8> {
    let mut payload = BytesMut::with_capacity(32 + title.len());
    payload.put_u128(uuid);
    write_varint(3, &mut payload); // Action 3: Update Title
    write_nbt_string(title, &mut payload);

    encode_packet(PACKET_ID_BOSS_EVENT, &payload)
}

/// Encodes an Action 4 (`Update Style`) BossBar packet.
#[must_use]
pub fn encode_bossbar_style(uuid: u128, color: BossBarColor, division: BossBarDivision) -> Vec<u8> {
    let mut payload = BytesMut::with_capacity(24);
    payload.put_u128(uuid);
    write_varint(4, &mut payload); // Action 4: Update Style
    write_varint(color as i32, &mut payload);
    write_varint(division as i32, &mut payload);

    encode_packet(PACKET_ID_BOSS_EVENT, &payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bossbar_packet_encodings() {
        let uuid = 0x1234_5678_9ABC_DEF0_1234_5678_9ABC_DEF0;
        let add_pkt = encode_bossbar_add(
            uuid,
            "{\"text\":\"Test\"}",
            0.75,
            BossBarColor::Blue,
            BossBarDivision::None,
        );
        assert!(!add_pkt.is_empty());

        let remove_pkt = encode_bossbar_remove(uuid);
        assert!(!remove_pkt.is_empty());

        let health_pkt = encode_bossbar_health(uuid, 0.5);
        assert!(!health_pkt.is_empty());

        let title_pkt = encode_bossbar_title(uuid, "{\"text\":\"New Title\"}");
        assert!(!title_pkt.is_empty());

        let style_pkt = encode_bossbar_style(uuid, BossBarColor::Purple, BossBarDivision::Notches6);
        assert!(!style_pkt.is_empty());
    }
}
