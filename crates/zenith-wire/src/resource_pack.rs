//! Minecraft Server Resource Pack packets for Protocol 777 (Java Edition 26.3).
//!
//! Handles background resource pack preloading (`ResourcePackPushPacket`) and
//! client download status acknowledgments (`ResourcePackResponsePacket`).

use crate::{WireError, read_varint, write_string};
use bytes::{Buf, BufMut, BytesMut};

/// Packet ID for `ClientboundResourcePackPushPacket` in Protocol 777 Play state.
pub const RESOURCE_PACK_PUSH_PLAY_ID: i32 = 82;

/// Packet ID for `ServerboundResourcePackPacket` in Protocol 777 Play state.
pub const RESOURCE_PACK_RESPONSE_PLAY_ID: i32 = 44;

/// Client response action status for a server resource pack request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourcePackAction {
    SuccessfullyLoaded = 0,
    Declined = 1,
    FailedDownload = 2,
    Accepted = 3,
    Downloaded = 4,
}

impl TryFrom<i32> for ResourcePackAction {
    type Error = WireError;

    fn try_from(val: i32) -> Result<Self, Self::Error> {
        match val {
            0 => Ok(Self::SuccessfullyLoaded),
            1 => Ok(Self::Declined),
            2 => Ok(Self::FailedDownload),
            3 => Ok(Self::Accepted),
            4 => Ok(Self::Downloaded),
            other => Err(WireError::InvalidIntent(other)),
        }
    }
}

/// Serverbound packet indicating the client's progress or decision on a resource pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourcePackResponse {
    pub uuid: u128,
    pub action: ResourcePackAction,
}

impl ResourcePackResponse {
    /// Encodes the response packet into bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = BytesMut::with_capacity(20);
        buf.put_u128(self.uuid);
        crate::write_varint(self.action as i32, &mut buf);
        buf.to_vec()
    }

    pub fn decode(payload: &[u8]) -> Result<Self, WireError> {
        if payload.len() < 16 {
            return Err(WireError::Underflow);
        }
        let mut buf = payload;
        let uuid = buf.get_u128();
        let action_raw = read_varint(&mut buf)?;
        let action = ResourcePackAction::try_from(action_raw)?;

        Ok(Self { uuid, action })
    }
}

/// Clientbound packet commanding the client to download and cache a server resource pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourcePackPush {
    pub uuid: u128,
    pub url: String,
    pub hash: String,
    pub required: bool,
    pub prompt: Option<String>,
}

impl ResourcePackPush {
    pub fn new(
        uuid: u128,
        url: impl Into<String>,
        hash: impl Into<String>,
        required: bool,
        prompt: Option<String>,
    ) -> Self {
        Self {
            uuid,
            url: url.into(),
            hash: hash.into(),
            required,
            prompt,
        }
    }

    /// Serializes the packet payload.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = BytesMut::new();
        buf.put_u128(self.uuid);
        write_string(&self.url, &mut buf);
        write_string(&self.hash, &mut buf);
        buf.put_u8(if self.required { 1 } else { 0 });

        if let Some(ref text) = self.prompt {
            buf.put_u8(1); // Has prompt component
            let json = format!(r#"{{"text":"{}"}}"#, text.replace('"', "\\\""));
            write_string(&json, &mut buf);
        } else {
            buf.put_u8(0); // No prompt component
        }

        buf.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_pack_push_encode() {
        let uuid = 0x12345678_1234_5678_1234_567812345678;
        let push = ResourcePackPush::new(
            uuid,
            "https://cdn.example.com/horizon.zip",
            "40a7a5f6e80b4352b217315a6b0c2a79c3b87834",
            false,
            Some("Downloading custom pack...".into()),
        );

        let encoded = push.encode();
        assert!(!encoded.is_empty());
        assert_eq!(&encoded[0..16], &uuid.to_be_bytes());
    }

    #[test]
    fn test_resource_pack_response_decode() {
        let uuid = 0xabcd_ef01_2345_6789_abcd_ef01_2345_6789;
        let mut buf = BytesMut::new();
        buf.put_u128(uuid);
        crate::write_varint(ResourcePackAction::SuccessfullyLoaded as i32, &mut buf);

        let resp = ResourcePackResponse::decode(&buf).unwrap();
        assert_eq!(resp.uuid, uuid);
        assert_eq!(resp.action, ResourcePackAction::SuccessfullyLoaded);
    }
}
