//! Low-level protocol framing, binary codecs, and VarInt serialization for Zenith.

use bytes::{Buf, BufMut};
use thiserror::Error;

/// Protocol 777 (Minecraft 26.3) wire errors.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum WireError {
    #[error("VarInt is too wide (exceeds 5 bytes)")]
    VarIntTooWide,
    #[error("Buffer underflow while reading packet")]
    Underflow,
    #[error("Packet payload exceeds maximum allowed size ({0} bytes)")]
    PayloadTooLarge(usize),
}

/// Reads a VarInt from a buffer slice.
pub fn read_varint(buf: &mut impl Buf) -> Result<i32, WireError> {
    let mut value = 0;
    let mut position = 0;

    while buf.has_remaining() {
        let current_byte = buf.get_u8();
        value |= ((current_byte & 0x7F) as i32) << position;

        if (current_byte & 0x80) == 0 {
            return Ok(value);
        }

        position += 7;
        if position >= 35 {
            return Err(WireError::VarIntTooWide);
        }
    }

    Err(WireError::Underflow)
}

/// Writes a VarInt into a buffer.
pub fn write_varint(mut value: i32, buf: &mut impl BufMut) {
    loop {
        if (value & !0x7F) == 0 {
            buf.put_u8(value as u8);
            return;
        } else {
            buf.put_u8(((value & 0x7F) | 0x80) as u8);
            value = (value as u32 >> 7) as i32;
        }
    }
}

/// Computes the serialized byte length of a VarInt.
#[must_use]
pub const fn varint_len(value: i32) -> usize {
    let v = value as u32;
    if v < (1 << 7) {
        1
    } else if v < (1 << 14) {
        2
    } else if v < (1 << 21) {
        3
    } else if v < (1 << 28) {
        4
    } else {
        5
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;

    #[test]
    fn test_varint_roundtrip() {
        let test_cases = [0, 1, 127, 128, 255, 25565, 2097151, -1, i32::MAX, i32::MIN];
        for &val in &test_cases {
            let mut buf = BytesMut::new();
            write_varint(val, &mut buf);
            assert_eq!(buf.len(), varint_len(val));
            let decoded = read_varint(&mut buf).expect("failed to decode varint");
            assert_eq!(decoded, val);
        }
    }

    #[test]
    fn test_varint_too_wide() {
        let mut buf = &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01][..];
        let err = read_varint(&mut buf);
        assert_eq!(err, Err(WireError::VarIntTooWide));
    }
}
