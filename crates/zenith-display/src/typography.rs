//! Small-caps typography conversion and time formatting.

/// Converts standard ASCII alphabetic characters into Unicode Small-Caps equivalents.
///
/// Characters not mapped to small-caps remain unchanged.
#[must_use]
pub fn to_small_caps(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 3);
    for c in input.chars() {
        let sc = match c.to_ascii_lowercase() {
            'a' => 'ᴀ',
            'b' => 'ʙ',
            'c' => 'ᴄ',
            'd' => 'ᴅ',
            'e' => 'ᴇ',
            'f' => 'ꜰ',
            'g' => 'ɢ',
            'h' => 'ʜ',
            'i' => 'ɪ',
            'j' => 'ᴊ',
            'k' => 'ᴋ',
            'l' => 'ʟ',
            'm' => 'ᴍ',
            'n' => 'ɴ',
            'o' => 'ᴏ',
            'p' => 'ᴘ',
            'q' => 'ǫ',
            'r' => 'ʀ',
            's' => 's',
            't' => 'ᴛ',
            'u' => 'ᴜ',
            'v' => 'ᴠ',
            'w' => 'ᴡ',
            'x' => 'x',
            'y' => 'ʏ',
            'z' => 'ᴢ',
            other => other,
        };
        out.push(sc);
    }
    out
}

/// Formats a duration in seconds into `mm:ss` display format.
#[must_use]
pub fn format_eta(seconds: u64) -> String {
    let mins = seconds / 60;
    let secs = seconds % 60;
    format!("{mins:02}:{secs:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_small_caps_conversion() {
        assert_eq!(to_small_caps("Queue"), "ǫᴜᴇᴜᴇ");
        assert_eq!(to_small_caps("Position"), "ᴘᴏsɪᴛɪᴏɴ");
        assert_eq!(to_small_caps("Est. Wait"), "ᴇsᴛ. ᴡᴀɪᴛ");
        assert_eq!(to_small_caps("123 !?"), "123 !?");
    }

    #[test]
    fn test_eta_formatting() {
        assert_eq!(format_eta(0), "00:00");
        assert_eq!(format_eta(9), "00:09");
        assert_eq!(format_eta(65), "01:05");
        assert_eq!(format_eta(3600), "60:00");
    }
}
