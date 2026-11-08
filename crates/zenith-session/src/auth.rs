//! Mojang session server authentication and verification client.

use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::time::Duration;
use thiserror::Error;

/// Standard Mojang session server verification endpoint.
pub const DEFAULT_MOJANG_AUTH_URL: &str =
    "https://sessionserver.mojang.com/session/minecraft/hasJoined";

/// Mojang game profile property (e.g. textures skin/cape and cryptographic signature).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MojangProperty {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub signature: Option<String>,
}

/// Verified Mojang game profile returned by `hasJoined`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MojangProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub properties: Vec<MojangProperty>,
}

impl MojangProfile {
    /// Parses the hexadecimal UUID string (both unhyphenated and RFC 4122) into a 128-bit integer.
    #[must_use]
    pub fn parsed_uuid(&self) -> Option<u128> {
        let clean = self.id.replace('-', "");
        u128::from_str_radix(&clean, 16).ok()
    }
}

/// Errors occurring during Mojang authentication.
#[derive(Debug, Error)]
pub enum AuthError {
    #[error("HTTP transport error: {0}")]
    Transport(String),
    #[error("Auth service returned HTTP status {0}")]
    HttpStatus(u16),
    #[error("Failed to parse JSON response: {0}")]
    InvalidJson(String),
    #[error("Authentication request timed out after {0:?}")]
    Timeout(Duration),
    #[error("Player has not joined server or verification was rejected")]
    NotJoined,
}

/// Queries the Mojang authentication server to verify whether `username` joined with `server_hash`.
///
/// When `prevent_proxy_connections` is enabled, the client's public IP address is passed to Mojang.
pub async fn verify_has_joined(
    endpoint: &str,
    username: &str,
    server_hash: &str,
    client_ip: Option<IpAddr>,
    timeout: Duration,
) -> Result<MojangProfile, AuthError> {
    let mut url = format!("{endpoint}?username={username}&serverId={server_hash}");
    if let Some(ip) = client_ip {
        url.push_str(&format!("&ip={ip}"));
    }

    let url_clone = url.clone();
    let res = tokio::task::spawn_blocking(move || {
        let agent = ureq::AgentBuilder::new().timeout(timeout).build();

        let response = match agent.get(&url_clone).call() {
            Ok(resp) => resp,
            Err(ureq::Error::Status(403 | 400 | 204, _)) => return Ok(None),
            Err(ureq::Error::Status(status, _)) => return Err(AuthError::HttpStatus(status)),
            Err(ureq::Error::Transport(t)) => return Err(AuthError::Transport(t.to_string())),
        };

        if response.status() == 204 {
            return Ok(None);
        }

        let body = response
            .into_string()
            .map_err(|e| AuthError::Transport(e.to_string()))?;

        if body.trim().is_empty() {
            return Ok(None);
        }

        let profile: MojangProfile = serde_json::from_str(&body)
            .map_err(|e| AuthError::InvalidJson(format!("{e}: {body}")))?;

        Ok(Some(profile))
    })
    .await
    .map_err(|e| AuthError::Transport(format!("Task spawn failed: {e}")))?;

    match res? {
        Some(profile) => Ok(profile),
        None => Err(AuthError::NotJoined),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mojang_profile_json() {
        let json = r#"{
            "id": "5f3c1e6a2b9d4c7e8a1f0b2c3d4e5f60",
            "name": "Axxio",
            "properties": [
                {
                    "name": "textures",
                    "value": "base64encodedtextures",
                    "signature": "signaturesample"
                }
            ]
        }"#;

        let profile: MojangProfile = serde_json::from_str(json).expect("failed to parse profile");
        assert_eq!(profile.name, "Axxio");
        assert_eq!(profile.id, "5f3c1e6a2b9d4c7e8a1f0b2c3d4e5f60");
        assert_eq!(profile.properties.len(), 1);
        assert_eq!(profile.properties[0].name, "textures");
        assert_eq!(
            profile.parsed_uuid(),
            Some(0x5f3c1e6a2b9d4c7e8a1f0b2c3d4e5f60)
        );
    }

    #[test]
    fn test_parse_hyphenated_uuid() {
        let profile = MojangProfile {
            id: "5f3c1e6a-2b9d-4c7e-8a1f-0b2c3d4e5f60".into(),
            name: "Tester".into(),
            properties: vec![],
        };
        assert_eq!(
            profile.parsed_uuid(),
            Some(0x5f3c1e6a2b9d4c7e8a1f0b2c3d4e5f60)
        );
    }
}
