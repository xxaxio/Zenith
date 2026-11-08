//! Client Behavioral & Protocol Fingerprinting (Anti-Bot Shield).
//!
//! Validates `ClientInformation` (Packet 0x00) and `minecraft:brand` plugin messages
//! during the Protocol 777 Configuration phase to detect and reject botnets, headless scripts,
//! and proxy flooders without false-flagging legitimate players or inspecting usernames.

use thiserror::Error;
use zenith_wire::ClientInformation;

/// Errors emitted when an incoming client fingerprint fails legitimacy validation.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FingerprintError {
    #[error("Missing mandatory ClientInformation packet during Configuration phase")]
    MissingClientInfo,
    #[error("Invalid client locale: {0:?}")]
    InvalidLocale(String),
    #[error("Illegal client view distance {0} (allowed: {1}..={2})")]
    InvalidViewDistance(i8, i8, i8),
    #[error("Invalid chat visibility enum: {0}")]
    InvalidChatVisibility(i32),
    #[error("Invalid main hand enum: {0}")]
    InvalidMainHand(i32),
    #[error("Missing mandatory client brand")]
    MissingBrand,
    #[error("Blocked client software brand: {0:?}")]
    BlockedBrand(String),
    #[error("Client brand exceeds allowed length ({0} bytes, max 64)")]
    BrandTooLong(usize),
}

/// Configuration settings for Client Behavioral & Protocol Fingerprinting.
#[derive(Debug, Clone)]
pub struct FingerprintConfig {
    /// Whether fingerprint validation is active.
    pub enabled: bool,
    /// Whether `ClientInformation` is mandatory before completing Configuration (true for all real clients).
    pub require_client_info: bool,
    /// Minimum acceptable view distance in chunks (vanilla minimum is 2).
    pub min_view_distance: i8,
    /// Maximum acceptable view distance in chunks (vanilla maximum is 32).
    pub max_view_distance: i8,
    /// Whether `minecraft:brand` is mandatory.
    pub require_brand: bool,
    /// Blacklisted client software brands (matched case-insensitively). Note: player usernames are never checked.
    pub blocked_brands: Vec<String>,
}

impl Default for FingerprintConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            require_client_info: true,
            min_view_distance: 2,
            max_view_distance: 32,
            require_brand: false,
            blocked_brands: vec![
                "mcbot".to_string(),
                "crafatar".to_string(),
                "botnet".to_string(),
                "headless".to_string(),
            ],
        }
    }
}

/// Accumulated protocol fingerprint collected during session handshake and configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientFingerprint {
    pub client_info: Option<ClientInformation>,
    pub brand: Option<String>,
}

/// Validator evaluating client fingerprint legitimacy against configured thresholds.
#[derive(Debug, Clone, Copy, Default)]
pub struct FingerprintValidator;

impl FingerprintValidator {
    /// Evaluates whether an accumulated client fingerprint satisfies security criteria.
    pub fn evaluate(
        config: &FingerprintConfig,
        fp: &ClientFingerprint,
    ) -> Result<(), FingerprintError> {
        if !config.enabled {
            return Ok(());
        }

        // 1. Validate ClientInformation
        if config.require_client_info {
            let Some(ref info) = fp.client_info else {
                return Err(FingerprintError::MissingClientInfo);
            };

            // Locale check: length 2..=16, ASCII alphanumeric with '_' or '-'
            let loc = &info.language;
            if loc.len() < 2
                || loc.len() > 16
                || !loc
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Err(FingerprintError::InvalidLocale(loc.clone()));
            }

            // View distance bound check
            if info.view_distance < config.min_view_distance
                || info.view_distance > config.max_view_distance
            {
                return Err(FingerprintError::InvalidViewDistance(
                    info.view_distance,
                    config.min_view_distance,
                    config.max_view_distance,
                ));
            }

            // Chat visibility enum: 0 (Full), 1 (System), 2 (Hidden)
            if !(0..=2).contains(&info.chat_visibility) {
                return Err(FingerprintError::InvalidChatVisibility(
                    info.chat_visibility,
                ));
            }

            // Main hand enum: 0 (Left), 1 (Right)
            if !(0..=1).contains(&info.main_hand) {
                return Err(FingerprintError::InvalidMainHand(info.main_hand));
            }
        }

        // 2. Validate Client Brand
        if config.require_brand && fp.brand.is_none() {
            return Err(FingerprintError::MissingBrand);
        }

        if let Some(ref brand) = fp.brand {
            if brand.len() > 64 {
                return Err(FingerprintError::BrandTooLong(brand.len()));
            }

            let brand_lower = brand.to_ascii_lowercase();
            for blocked in &config.blocked_brands {
                if !blocked.is_empty() && brand_lower.contains(&blocked.to_ascii_lowercase()) {
                    return Err(FingerprintError::BlockedBrand(brand.clone()));
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_vanilla_fingerprint_passes() {
        let config = FingerprintConfig::default();
        let fp = ClientFingerprint {
            client_info: Some(ClientInformation {
                language: "fr_fr".into(),
                view_distance: 12,
                chat_visibility: 0,
                chat_colors: true,
                model_customisation: 0x7F,
                main_hand: 1,
                text_filtering: false,
                allows_listing: true,
                particle_status: 0,
            }),
            brand: Some("vanilla".into()),
        };

        assert_eq!(FingerprintValidator::evaluate(&config, &fp), Ok(()));
    }

    #[test]
    fn test_missing_client_info_rejected() {
        let config = FingerprintConfig::default();
        let fp = ClientFingerprint {
            client_info: None,
            brand: Some("vanilla".into()),
        };

        assert_eq!(
            FingerprintValidator::evaluate(&config, &fp),
            Err(FingerprintError::MissingClientInfo)
        );
    }

    #[test]
    fn test_illegal_view_distance_rejected() {
        let config = FingerprintConfig::default();

        let mut fp = ClientFingerprint {
            client_info: Some(ClientInformation::default()),
            brand: Some("vanilla".into()),
        };

        // 0 chunks is physically impossible in vanilla client
        fp.client_info.as_mut().unwrap().view_distance = 0;
        assert_eq!(
            FingerprintValidator::evaluate(&config, &fp),
            Err(FingerprintError::InvalidViewDistance(0, 2, 32))
        );

        // 100 chunks exceeds vanilla maximum (32)
        fp.client_info.as_mut().unwrap().view_distance = 100;
        assert_eq!(
            FingerprintValidator::evaluate(&config, &fp),
            Err(FingerprintError::InvalidViewDistance(100, 2, 32))
        );
    }

    #[test]
    fn test_blocked_client_brand_rejected() {
        let config = FingerprintConfig::default();
        let fp = ClientFingerprint {
            client_info: Some(ClientInformation::default()),
            brand: Some("mcbot-cluster-v2".into()),
        };

        assert_eq!(
            FingerprintValidator::evaluate(&config, &fp),
            Err(FingerprintError::BlockedBrand("mcbot-cluster-v2".into()))
        );
    }

    #[test]
    fn test_invalid_locale_rejected() {
        let config = FingerprintConfig::default();
        let mut fp = ClientFingerprint {
            client_info: Some(ClientInformation::default()),
            brand: Some("vanilla".into()),
        };

        fp.client_info.as_mut().unwrap().language = "inv@lid!".into();
        assert_eq!(
            FingerprintValidator::evaluate(&config, &fp),
            Err(FingerprintError::InvalidLocale("inv@lid!".into()))
        );
    }

    #[test]
    fn test_disabled_config_bypasses_all_checks() {
        let config = FingerprintConfig {
            enabled: false,
            ..Default::default()
        };

        let empty_fp = ClientFingerprint::default();
        assert_eq!(FingerprintValidator::evaluate(&config, &empty_fp), Ok(()));
    }
}
