//! 24-bit True RGB color interpolation and gradient formatting for Minecraft Protocol 777.

use crate::component::ChatComponent;

/// Precomputed legacy RGB brand string for `ᴢᴇɴɪᴛʜ` with 24-bit Solar Ember gradient (#FF3700 -> #FFAA00).
pub const BRAND_LOGO_LEGACY: &str = "§x§f§f§3§7§0§0§lᴢ§x§f§f§4§e§0§0§lᴇ§x§f§f§6§5§0§0§lɴ§x§f§f§7§c§0§0§lɪ§x§f§f§9§3§0§0§lᴛ§x§f§f§a§a§0§0§lʜ";

/// Precomputed chat prefix: `<gradient>ᴢᴇɴɪᴛʜ</gradient> §8»`
pub const BRAND_PREFIX: &str = "§x§f§f§3§7§0§0§lᴢ§x§f§f§4§e§0§0§lᴇ§x§f§f§6§5§0§0§lɴ§x§f§f§7§c§0§0§lɪ§x§f§f§9§3§0§0§lᴛ§x§f§f§a§a§0§0§lʜ §8»";

/// Precomputed spaced title logo for welcome titles: `ᴢ ᴇ ɴ ɪ ᴛ ʜ` with wide radiant gradient (#FF3700 -> #FFD000).
pub const BRAND_TITLE_LEGACY: &str = "§x§f§f§3§7§0§0§lᴢ §x§f§f§5§5§0§0§lᴇ §x§f§f§7§4§0§0§lɴ §x§f§f§9§2§0§0§lɪ §x§f§f§b§1§0§0§lᴛ §x§f§f§d§0§0§0§lʜ";

/// Represents an RGB color with 8-bit channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbColor(pub u8, pub u8, pub u8);

impl RgbColor {
    /// Parses a hexadecimal color code (e.g. `"#FF3700"` or `"FF3700"`).
    #[must_use]
    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.trim().trim_start_matches('#');
        if hex.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some(Self(r, g, b))
    }

    /// Formats the color into a standard hex string (e.g. `"#ff3700"`).
    #[must_use]
    pub fn to_hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }

    /// Formats the color into Minecraft 1.16+ legacy section-sign format:
    /// `§x§r§r§g§g§b§b`
    #[must_use]
    pub fn to_legacy_code(&self) -> String {
        format!(
            "§x§{:x}§{:x}§{:x}§{:x}§{:x}§{:x}",
            (self.0 >> 4) & 0xF,
            self.0 & 0xF,
            (self.1 >> 4) & 0xF,
            self.1 & 0xF,
            (self.2 >> 4) & 0xF,
            self.2 & 0xF
        )
    }

    /// Linearly interpolates between two colors with factor `t` in `[0.0, 1.0]`.
    #[must_use]
    pub fn interpolate(c1: Self, c2: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let r = (c1.0 as f32 + t * (c2.0 as f32 - c1.0 as f32)).round() as u8;
        let g = (c1.1 as f32 + t * (c2.1 as f32 - c1.1 as f32)).round() as u8;
        let b = (c1.2 as f32 + t * (c2.2 as f32 - c1.2 as f32)).round() as u8;
        Self(r, g, b)
    }
}

/// Generates a Minecraft legacy string with a 24-bit RGB gradient applied to each character.
///
/// Output format: `§x§r§r§g§g§b§b[§l]c...`
#[must_use]
pub fn gradient_legacy_string(text: &str, start_hex: &str, end_hex: &str, bold: bool) -> String {
    let start = RgbColor::from_hex(start_hex).unwrap_or(RgbColor(255, 62, 0));
    let end = RgbColor::from_hex(end_hex).unwrap_or(RgbColor(255, 208, 0));
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();

    if n == 0 {
        return String::new();
    }

    let mut out = String::with_capacity(n * 16);
    for (i, &ch) in chars.iter().enumerate() {
        let t = if n > 1 {
            i as f32 / (n - 1) as f32
        } else {
            0.0
        };
        let color = RgbColor::interpolate(start, end, t);
        out.push_str(&color.to_legacy_code());
        if bold {
            out.push_str("§l");
        }
        out.push(ch);
    }
    out
}

/// Generates a multi-stop Minecraft legacy string across an array of hex stops.
#[must_use]
pub fn multi_gradient_legacy_string(text: &str, hex_stops: &[&str], bold: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    if n == 0 {
        return String::new();
    }
    if hex_stops.len() < 2 {
        let color = hex_stops
            .first()
            .and_then(|h| RgbColor::from_hex(h))
            .unwrap_or(RgbColor(255, 170, 0));
        let code = color.to_legacy_code();
        let bold_code = if bold { "§l" } else { "" };
        return format!("{code}{bold_code}{text}");
    }

    let parsed_stops: Vec<RgbColor> = hex_stops
        .iter()
        .map(|h| RgbColor::from_hex(h).unwrap_or(RgbColor(255, 255, 255)))
        .collect();

    let num_segments = (parsed_stops.len() - 1) as f32;
    let mut out = String::with_capacity(n * 16);

    for (i, &ch) in chars.iter().enumerate() {
        let global_t = if n > 1 {
            i as f32 / (n - 1) as f32
        } else {
            0.0
        };
        let scaled_t = global_t * num_segments;
        let segment_idx = (scaled_t.floor() as usize).min(parsed_stops.len() - 2);
        let segment_t = scaled_t - segment_idx as f32;

        let color = RgbColor::interpolate(
            parsed_stops[segment_idx],
            parsed_stops[segment_idx + 1],
            segment_t,
        );
        out.push_str(&color.to_legacy_code());
        if bold {
            out.push_str("§l");
        }
        out.push(ch);
    }
    out
}

/// Generates a vector of JSON `ChatComponent`s with interpolated hex colors.
#[must_use]
pub fn gradient_components(
    text: &str,
    start_hex: &str,
    end_hex: &str,
    bold: bool,
) -> Vec<ChatComponent> {
    let start = RgbColor::from_hex(start_hex).unwrap_or(RgbColor(255, 62, 0));
    let end = RgbColor::from_hex(end_hex).unwrap_or(RgbColor(255, 208, 0));
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();

    if n == 0 {
        return Vec::new();
    }

    let mut out = Vec::with_capacity(n);
    for (i, &ch) in chars.iter().enumerate() {
        let t = if n > 1 {
            i as f32 / (n - 1) as f32
        } else {
            0.0
        };
        let color = RgbColor::interpolate(start, end, t);
        out.push(ChatComponent::styled(ch.to_string(), color.to_hex(), bold));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_parsing() {
        let col = RgbColor::from_hex("#ff3700").unwrap();
        assert_eq!(col, RgbColor(255, 55, 0));

        let col_no_hash = RgbColor::from_hex("00ff87").unwrap();
        assert_eq!(col_no_hash, RgbColor(0, 255, 135));

        assert!(RgbColor::from_hex("invalid").is_none());
        assert!(RgbColor::from_hex("#12").is_none());
    }

    #[test]
    fn test_legacy_code_formatting() {
        let col = RgbColor(255, 170, 0); // #ffaa00
        assert_eq!(col.to_legacy_code(), "§x§f§f§a§a§0§0");
    }

    #[test]
    fn test_interpolation() {
        let c1 = RgbColor(0, 0, 0);
        let c2 = RgbColor(100, 200, 50);

        assert_eq!(RgbColor::interpolate(c1, c2, 0.0), c1);
        assert_eq!(RgbColor::interpolate(c1, c2, 1.0), c2);
        assert_eq!(RgbColor::interpolate(c1, c2, 0.5), RgbColor(50, 100, 25));
    }

    #[test]
    fn test_gradient_legacy_string() {
        let res = gradient_legacy_string("ZENITH", "#FF0000", "#0000FF", true);
        assert!(!res.is_empty());
        assert!(res.contains("§x§f§f§0§0§0§0§lZ"));
        assert!(res.contains("§x§0§0§0§0§f§f§lH"));
    }

    #[test]
    fn test_multi_gradient_legacy() {
        let stops = ["#FF0000", "#00FF00", "#0000FF"];
        let res = multi_gradient_legacy_string("TESTING", &stops, false);
        assert!(!res.is_empty());
        assert!(res.contains("§x"));
    }
}
