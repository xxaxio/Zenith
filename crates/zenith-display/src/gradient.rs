//! 24-bit True RGB color interpolation and gradient formatting for Minecraft Protocol 777.

use crate::component::ChatComponent;

/// Precomputed legacy RGB brand string for `ᴢᴇɴɪᴛʜ` with 24-bit Solar Ember gradient (#FF3700 -> #FFAA00).
pub const BRAND_LOGO_LEGACY: &str = "§c§lᴢ§6§lᴇ§6§lɴ§e§lɪ§e§lᴛ§e§lʜ";

/// Precomputed chat prefix: `<gradient>ᴢᴇɴɪᴛʜ</gradient> §8»`
pub const BRAND_PREFIX: &str = "§c§lᴢ§6§lᴇ§6§lɴ§e§lɪ§e§lᴛ§e§lʜ §8»";

/// Precomputed spaced title logo for welcome titles: `ᴢ ᴇ ɴ ɪ ᴛ ʜ` with wide radiant gradient (#FF3700 -> #FFD000).
pub const BRAND_TITLE_LEGACY: &str = "§c§lᴢ §6§lᴇ §6§lɴ §e§lɪ §e§lᴛ §e§lʜ";

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

    /// Converts this color to the most visually accurate Minecraft 16-color legacy code (§0-§f).
    /// This prevents modern clients from misinterpreting legacy hex codes (§x) into black or green.
    #[must_use]
    pub fn to_vanilla_code(&self) -> &'static str {
        // Solar Ember palette specific mappings:
        // Deep Magma / Crimson Red (#FF3700 - #FF5500) -> §c
        if self.0 > 220 && self.1 < 80 && self.2 < 80 {
            return "§c";
        }
        // Vibrant Gold / Warm Amber (#FFAA00 - #FF8800) -> §6
        if self.0 > 220 && self.1 >= 80 && self.1 < 190 && self.2 < 80 {
            return "§6";
        }
        // Solar Yellow (#FFD000 - #FFFF55) -> §e
        if self.0 > 200 && self.1 >= 190 && self.2 < 120 {
            return "§e";
        }
        // Silver / Cool Gray (#AAAAAA) -> §7
        if self.0 >= 140
            && self.0 <= 190
            && self.1 >= 140
            && self.1 <= 190
            && self.2 >= 140
            && self.2 <= 190
        {
            return "§7";
        }
        // Steel / Dark Gray (#555555 - #777777) -> §8
        if self.0 >= 60
            && self.0 < 140
            && self.1 >= 60
            && self.1 < 140
            && self.2 >= 60
            && self.2 < 140
        {
            return "§8";
        }
        // Pure White (#FFFFFF) -> §f
        if self.0 > 230 && self.1 > 230 && self.2 > 230 {
            return "§f";
        }

        // Palette fallback
        let palette: [(RgbColor, &'static str); 16] = [
            (RgbColor(0, 0, 0), "§0"),
            (RgbColor(0, 0, 170), "§1"),
            (RgbColor(0, 170, 0), "§2"),
            (RgbColor(0, 170, 170), "§3"),
            (RgbColor(170, 0, 0), "§4"),
            (RgbColor(170, 0, 170), "§5"),
            (RgbColor(255, 170, 0), "§6"),
            (RgbColor(170, 170, 170), "§7"),
            (RgbColor(85, 85, 85), "§8"),
            (RgbColor(85, 85, 255), "§9"),
            (RgbColor(85, 255, 85), "§a"),
            (RgbColor(85, 255, 255), "§b"),
            (RgbColor(255, 85, 85), "§c"),
            (RgbColor(255, 85, 255), "§d"),
            (RgbColor(255, 255, 85), "§e"),
            (RgbColor(255, 255, 255), "§f"),
        ];

        let mut closest = "§f";
        let mut min_dist = u32::MAX;
        for (col, code) in &palette {
            let dr = self.0 as i32 - col.0 as i32;
            let dg = self.1 as i32 - col.1 as i32;
            let db = self.2 as i32 - col.2 as i32;
            let dist = (dr * dr + dg * dg + db * db) as u32;
            if dist < min_dist {
                min_dist = dist;
                closest = code;
            }
        }
        closest
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
    fn test_vanilla_code_mapping() {
        assert_eq!(RgbColor(255, 55, 0).to_vanilla_code(), "§c"); // Magma
        assert_eq!(RgbColor(255, 170, 0).to_vanilla_code(), "§6"); // Gold
        assert_eq!(RgbColor(255, 208, 0).to_vanilla_code(), "§e"); // Yellow
        assert_eq!(RgbColor(170, 170, 170).to_vanilla_code(), "§7"); // Silver
        assert_eq!(RgbColor(85, 85, 85).to_vanilla_code(), "§8"); // Steel
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
