//! Cryptographic HMAC-SHA256 forwarding signatures and anti-spoofing verification for Zenith.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Provides cryptographic HMAC-SHA256 forwarding token generation and verification.
#[derive(Debug, Clone)]
pub struct HmacForwarding {
    secret: Vec<u8>,
}

impl HmacForwarding {
    /// Creates a new HMAC signer/verifier with the specified shared secret.
    #[must_use]
    pub fn new(secret: impl Into<Vec<u8>>) -> Self {
        Self {
            secret: secret.into(),
        }
    }

    /// Computes the HMAC-SHA256 signature over an arbitrary data slice.
    #[must_use]
    pub fn sign(&self, data: &[u8]) -> [u8; 32] {
        let mut mac =
            HmacSha256::new_from_slice(&self.secret).expect("HMAC can take key of any size");
        mac.update(data);
        mac.finalize().into_bytes().into()
    }

    /// Verifies an HMAC-SHA256 signature in constant time against expected bytes.
    #[must_use]
    pub fn verify(&self, data: &[u8], expected: &[u8]) -> bool {
        let mut mac = match HmacSha256::new_from_slice(&self.secret) {
            Ok(m) => m,
            Err(_) => return false,
        };
        mac.update(data);
        mac.verify_slice(expected).is_ok()
    }

    /// Generates a signed forwarding token for a player transit session.
    #[must_use]
    pub fn create_transit_token(
        &self,
        uuid: u128,
        username: &str,
        timestamp_secs: u64,
    ) -> [u8; 32] {
        let mut payload = Vec::with_capacity(32 + username.len());
        payload.extend_from_slice(&uuid.to_be_bytes());
        payload.extend_from_slice(username.as_bytes());
        payload.extend_from_slice(&timestamp_secs.to_be_bytes());
        self.sign(&payload)
    }

    /// Verifies a player transit token, enforcing a maximum drift window.
    #[must_use]
    pub fn verify_transit_token(
        &self,
        uuid: u128,
        username: &str,
        timestamp_secs: u64,
        now_secs: u64,
        max_drift_secs: u64,
        signature: &[u8],
    ) -> bool {
        if now_secs.abs_diff(timestamp_secs) > max_drift_secs {
            return false;
        }

        let mut payload = Vec::with_capacity(32 + username.len());
        payload.extend_from_slice(&uuid.to_be_bytes());
        payload.extend_from_slice(username.as_bytes());
        payload.extend_from_slice(&timestamp_secs.to_be_bytes());
        self.verify(&payload, signature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hmac_sign_and_verify() {
        let hmac = HmacForwarding::new("zenith_super_secret_key_2026");
        let data = b"player:Axxio:uuid:123456";
        let sig = hmac.sign(data);

        assert!(hmac.verify(data, &sig));
        assert!(!hmac.verify(b"player:Impostor:uuid:123456", &sig));
    }

    #[test]
    fn test_transit_token_drift_and_tampering() {
        let hmac = HmacForwarding::new("secret_key");
        let uuid = 0x1234_5678_9ABC_DEF0_1234_5678_9ABC_DEF0;
        let user = "Player1";
        let timestamp = 1_700_000_000;

        let token = hmac.create_transit_token(uuid, user, timestamp);

        // Valid within 10 seconds drift
        assert!(hmac.verify_transit_token(uuid, user, timestamp, timestamp + 5, 10, &token));

        // Expired (drift exceeds 10 seconds)
        assert!(!hmac.verify_transit_token(uuid, user, timestamp, timestamp + 15, 10, &token));

        // Tampered user
        assert!(!hmac.verify_transit_token(uuid, "Hacker", timestamp, timestamp + 2, 10, &token));
    }
}
