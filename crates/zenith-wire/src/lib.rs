//! Low-level protocol framing, binary codecs, and VarInt serialization for Zenith.

use bytes::{Buf, BufMut, BytesMut};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub use horizon_protocol::PROTOCOL_VERSION;

pub mod hmac;
pub use hmac::HmacForwarding;

/// Wire protocol errors.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum WireError {
    #[error("VarInt is too wide (exceeds 5 bytes)")]
    VarIntTooWide,
    #[error("Buffer underflow while reading packet")]
    Underflow,
    #[error("Packet payload exceeds maximum allowed size ({0} bytes)")]
    PayloadTooLarge(usize),
    #[error("String exceeds maximum allowed length")]
    StringTooLong,
    #[error("Invalid UTF-8 string")]
    InvalidUtf8,
    #[error("Invalid state/intent ID: {0}")]
    InvalidIntent(i32),
    #[error("Invalid Minecraft username: {0:?}")]
    InvalidUsername(String),
    #[error("IO error: {0}")]
    Io(String),
}

impl From<std::io::Error> for WireError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

/// Reads a VarInt from a buffer slice with a 1-cycle fast path for single-byte values.
pub fn read_varint(buf: &mut impl Buf) -> Result<i32, WireError> {
    if !buf.has_remaining() {
        return Err(WireError::Underflow);
    }
    let first = buf.get_u8();
    if (first & 0x80) == 0 {
        return Ok(first as i32);
    }

    let mut value = (first & 0x7F) as i32;
    let mut position = 7;

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

/// Reads a VarInt asynchronously from a stream with a fast path for single-byte values.
pub async fn read_varint_async<R: AsyncRead + Unpin>(reader: &mut R) -> Result<i32, WireError> {
    let mut byte = [0u8; 1];
    reader.read_exact(&mut byte).await?;
    let first = byte[0];
    if (first & 0x80) == 0 {
        return Ok(first as i32);
    }

    let mut value = (first & 0x7F) as i32;
    let mut position = 7;

    loop {
        reader.read_exact(&mut byte).await?;
        let current_byte = byte[0];
        value |= ((current_byte & 0x7F) as i32) << position;

        if (current_byte & 0x80) == 0 {
            return Ok(value);
        }

        position += 7;
        if position >= 35 {
            return Err(WireError::VarIntTooWide);
        }
    }
}

/// Writes a VarInt into a buffer with a 1-cycle fast path for single-byte values.
pub fn write_varint(mut value: i32, buf: &mut impl BufMut) {
    if (value & !0x7F) == 0 {
        buf.put_u8(value as u8);
        return;
    }

    loop {
        if (value & !0x7F) == 0 {
            buf.put_u8(value as u8);
            return;
        }
        buf.put_u8(((value & 0x7F) | 0x80) as u8);
        value = (value as u32 >> 7) as i32;
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

/// Reads a length-prefixed UTF-8 string from a buffer.
pub fn read_string(buf: &mut impl Buf, max_len: usize) -> Result<String, WireError> {
    let len = read_varint(buf)? as usize;
    if len > max_len * 4 {
        return Err(WireError::StringTooLong);
    }
    if buf.remaining() < len {
        return Err(WireError::Underflow);
    }

    let mut bytes = vec![0u8; len];
    buf.copy_to_slice(&mut bytes);
    let s = String::from_utf8(bytes).map_err(|_| WireError::InvalidUtf8)?;
    if s.chars().count() > max_len {
        return Err(WireError::StringTooLong);
    }
    Ok(s)
}

/// Writes a length-prefixed UTF-8 string into a buffer.
pub fn write_string(s: &str, buf: &mut impl BufMut) {
    write_varint(s.len() as i32, buf);
    buf.put_slice(s.as_bytes());
}

/// Writes a standard Minecraft 26.3 / Protocol 777 Network NBT string tag (`TAG_String`).
pub fn write_nbt_string(s: &str, buf: &mut impl BufMut) {
    buf.put_u8(8); // TAG_String
    buf.put_u16(s.len() as u16);
    buf.put_slice(s.as_bytes());
}

/// Reads a raw Minecraft frame from an async stream: `[TotalLength: VarInt] [PacketID: VarInt] [Payload]`
pub async fn read_raw_packet<R: AsyncRead + Unpin>(
    reader: &mut R,
    max_length: usize,
) -> Result<(i32, Vec<u8>), WireError> {
    let total_len = read_varint_async(reader).await? as usize;
    if total_len > max_length {
        return Err(WireError::PayloadTooLarge(total_len));
    }
    if total_len == 0 {
        return Err(WireError::Underflow);
    }

    let mut packet_bytes = vec![0u8; total_len];
    reader.read_exact(&mut packet_bytes).await?;

    let mut slice = &packet_bytes[..];
    let packet_id = read_varint(&mut slice)?;
    let payload = slice.to_vec();

    Ok((packet_id, payload))
}

/// Writes a raw Minecraft frame to an async stream.
pub async fn write_raw_packet<W: AsyncWrite + Unpin>(
    writer: &mut W,
    packet_id: i32,
    payload: &[u8],
) -> Result<(), WireError> {
    let id_len = varint_len(packet_id);
    let total_len = id_len + payload.len();

    let mut header = BytesMut::with_capacity(varint_len(total_len as i32) + id_len);
    write_varint(total_len as i32, &mut header);
    write_varint(packet_id, &mut header);

    writer.write_all(&header).await?;
    if !payload.is_empty() {
        writer.write_all(payload).await?;
    }
    writer.flush().await?;
    Ok(())
}

/// Encodes an in-memory packet frame: `[TotalLength: VarInt] [PacketID: VarInt] [Payload]`
#[must_use]
pub fn encode_packet(packet_id: i32, payload: &[u8]) -> Vec<u8> {
    let id_len = varint_len(packet_id);
    let total_len = id_len + payload.len();
    let mut buf = BytesMut::with_capacity(varint_len(total_len as i32) + total_len);
    write_varint(total_len as i32, &mut buf);
    write_varint(packet_id, &mut buf);
    buf.put_slice(payload);
    buf.to_vec()
}

/// Handshake connection intent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandshakeIntent {
    Status = 1,
    Login = 2,
    Transfer = 3,
}

impl TryFrom<i32> for HandshakeIntent {
    type Error = WireError;

    fn try_from(val: i32) -> Result<Self, Self::Error> {
        match val {
            1 => Ok(Self::Status),
            2 => Ok(Self::Login),
            3 => Ok(Self::Transfer),
            other => Err(WireError::InvalidIntent(other)),
        }
    }
}

/// Minecraft Handshake packet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Handshake {
    pub protocol_version: i32,
    pub server_address: String,
    pub server_port: u16,
    pub intent: HandshakeIntent,
}

impl Handshake {
    pub fn decode(payload: &[u8]) -> Result<Self, WireError> {
        let mut buf = payload;
        let protocol_version = read_varint(&mut buf)?;
        let server_address = read_string(&mut buf, 255)?;
        if buf.remaining() < 2 {
            return Err(WireError::Underflow);
        }
        let server_port = buf.get_u16();
        let intent_raw = read_varint(&mut buf)?;
        let intent = HandshakeIntent::try_from(intent_raw)?;

        Ok(Self {
            protocol_version,
            server_address,
            server_port,
            intent,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut buf = BytesMut::new();
        write_varint(self.protocol_version, &mut buf);
        write_string(&self.server_address, &mut buf);
        buf.put_u16(self.server_port);
        write_varint(self.intent as i32, &mut buf);
        buf.to_vec()
    }
}

/// Validates whether a string is a standard Minecraft username:
/// 1 to 16 ASCII alphanumeric characters or underscores (`^[a-zA-Z0-9_]{1,16}$`).
#[must_use]
pub fn is_valid_minecraft_username(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 16
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Login Start packet (Packet ID 0x00 in Login state).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginStart {
    pub name: String,
    pub uuid: u128,
}

impl LoginStart {
    pub fn decode(payload: &[u8]) -> Result<Self, WireError> {
        let mut buf = payload;
        let name = read_string(&mut buf, 16)?;
        if !is_valid_minecraft_username(&name) {
            return Err(WireError::InvalidUsername(name));
        }
        let uuid = if buf.remaining() >= 16 {
            buf.get_u128()
        } else {
            0
        };
        Ok(Self { name, uuid })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut buf = BytesMut::new();
        write_string(&self.name, &mut buf);
        buf.put_u128(self.uuid);
        buf.to_vec()
    }
}

/// Login Success packet (Packet ID 0x02 in Login state).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginSuccess {
    pub uuid: u128,
    pub username: String,
}

impl LoginSuccess {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = BytesMut::new();
        buf.put_u128(self.uuid);
        write_string(&self.username, &mut buf);
        write_varint(0, &mut buf); // 0 properties
        buf.put_u128(self.uuid); // Protocol 777 session UUID (16 bytes)
        buf.to_vec()
    }
}

/// KeepAlive packet (ping/pong).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeepAlive {
    pub id: i64,
}

impl KeepAlive {
    pub fn decode(payload: &[u8]) -> Result<Self, WireError> {
        let mut buf = payload;
        if buf.remaining() < 8 {
            return Err(WireError::Underflow);
        }
        Ok(Self { id: buf.get_i64() })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut buf = BytesMut::with_capacity(8);
        buf.put_i64(self.id);
        buf.to_vec()
    }
}

/// Generates a deterministic offline player UUID from a player username.
pub fn offline_uuid(username: &str) -> u128 {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"OfflinePlayer:");
    hasher.update(username.as_bytes());
    let hash = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hash[..16]);
    // Set version to 3 and RFC 4122 variant
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    u128::from_be_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_handshake_roundtrip() {
        let hs = Handshake {
            protocol_version: 777,
            server_address: "play.horizon.dev".into(),
            server_port: 25565,
            intent: HandshakeIntent::Login,
        };
        let encoded = hs.encode();
        let decoded = Handshake::decode(&encoded).expect("failed to decode handshake");
        assert_eq!(decoded, hs);
    }

    #[test]
    fn test_login_start_roundtrip() {
        let start = LoginStart {
            name: "Steve".into(),
            uuid: 0x1234_5678_9ABC_DEF0_1234_5678_9ABC_DEF0,
        };
        let encoded = start.encode();
        let decoded = LoginStart::decode(&encoded).expect("failed to decode login start");
        assert_eq!(decoded, start);
    }

    #[tokio::test]
    async fn test_raw_packet_io() {
        let mut stream = Vec::new();
        let payload = b"Hello, Zenith!";
        write_raw_packet(&mut stream, 0x42, payload).await.unwrap();

        let mut reader = &stream[..];
        let (id, read_payload) = read_raw_packet(&mut reader, 1024).await.unwrap();
        assert_eq!(id, 0x42);
        assert_eq!(&read_payload, payload);
    }

    #[test]
    fn test_minecraft_username_validation() {
        assert!(is_valid_minecraft_username("Steve"));
        assert!(is_valid_minecraft_username("Axxio_HC"));
        assert!(is_valid_minecraft_username("Player_123"));
        assert!(is_valid_minecraft_username("a"));
        assert!(is_valid_minecraft_username("1234567890123456"));

        // Invalid names
        assert!(!is_valid_minecraft_username(""));
        assert!(!is_valid_minecraft_username("12345678901234567")); // 17 chars
        assert!(!is_valid_minecraft_username("bot\nquit\n"));
        assert!(!is_valid_minecraft_username("user name"));
        assert!(!is_valid_minecraft_username("player!"));
        assert!(!is_valid_minecraft_username("héllò"));
    }

    #[test]
    fn test_login_start_rejects_injected_newlines() {
        let mut buf = bytes::BytesMut::new();
        write_string("bot\nquit\n", &mut buf);
        buf.put_u128(0x42);
        let err = LoginStart::decode(&buf).unwrap_err();
        assert!(matches!(err, WireError::InvalidUsername(_)));
    }
}
