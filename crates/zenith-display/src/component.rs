//! Minecraft Protocol 777 JSON Text Component serialization.

use crate::config::DisplayConfig;
use crate::gradient::{RgbColor, gradient_components};
use crate::typography::{format_eta, to_small_caps};
use serde::{Deserialize, Serialize};

/// A Minecraft JSON text component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatComponent {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub extra: Vec<ChatComponent>,
}

impl ChatComponent {
    /// Creates a plain text component.
    #[must_use]
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            color: None,
            bold: None,
            italic: None,
            extra: Vec::new(),
        }
    }

    /// Creates a styled text component with optional hex color and boldness.
    #[must_use]
    pub fn styled(text: impl Into<String>, color: impl Into<String>, bold: bool) -> Self {
        Self {
            text: text.into(),
            color: Some(color.into()),
            bold: if bold { Some(true) } else { None },
            italic: None,
            extra: Vec::new(),
        }
    }

    /// Appends a child component to the `extra` list.
    pub fn add_extra(&mut self, child: ChatComponent) -> &mut Self {
        self.extra.push(child);
        self
    }

    /// Serializes the component into standard Minecraft JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| format!("{{\"text\":\"{}\"}}", self.text))
    }

    /// Serializes the component into Minecraft section-sign (§) formatted legacy text for network NBT.
    ///
    /// Supports both standard 16 colors and 24-bit True RGB hex codes (§x§r§r§g§g§b§b).
    #[must_use]
    pub fn to_legacy_string(&self) -> String {
        let mut out = String::new();
        fn append_comp(c: &ChatComponent, out: &mut String) {
            if let Some(col) = &c.color {
                if let Some(rgb) = RgbColor::from_hex(col) {
                    out.push_str(&rgb.to_legacy_code());
                } else {
                    match col.as_str() {
                        "red" => out.push_str("§c"),
                        "gold" => out.push_str("§6"),
                        "yellow" => out.push_str("§e"),
                        "green" => out.push_str("§a"),
                        "aqua" | "cyan" => out.push_str("§b"),
                        "blue" => out.push_str("§9"),
                        "purple" => out.push_str("§5"),
                        "gray" => out.push_str("§7"),
                        "dark_gray" => out.push_str("§8"),
                        "white" => out.push_str("§f"),
                        _ => out.push_str("§f"),
                    }
                }
            }
            if c.bold == Some(true) {
                out.push_str("§l");
            }
            if c.italic == Some(true) {
                out.push_str("§o");
            }
            out.push_str(&c.text);
            for sub in &c.extra {
                append_comp(sub, out);
            }
        }
        append_comp(self, &mut out);
        out
    }
}

/// Builds the formatted BossBar title component according to the display configuration.
#[must_use]
pub fn build_bossbar_title(
    config: &DisplayConfig,
    title_text: &str,
    position: usize,
    total: usize,
    eta_seconds: u64,
) -> ChatComponent {
    let title = if config.small_caps {
        to_small_caps(title_text)
    } else {
        title_text.to_string()
    };
    let pos_label = if config.small_caps {
        to_small_caps(&config.position_label)
    } else {
        config.position_label.clone()
    };
    let eta_label = if config.small_caps {
        to_small_caps(&config.eta_label)
    } else {
        config.eta_label.clone()
    };

    let mut root = ChatComponent::plain("");

    // Section 1: Header / Title in vibrant Solar Amber RGB gradient
    for comp in gradient_components(&title, &config.gradient_start, &config.gradient_end, true) {
        root.add_extra(comp);
    }

    // Separator (fine gray)
    root.add_extra(ChatComponent::styled(" §8│ ", "#555555", false));

    // Section 2: Position (Label in cool gray, numbers in amber highlight)
    root.add_extra(ChatComponent::styled(
        format!("{pos_label}: "),
        "#aaaaaa",
        false,
    ));
    root.add_extra(ChatComponent::styled(
        format!("#{position}"),
        &config.gradient_end,
        true,
    ));
    root.add_extra(ChatComponent::styled(" §8/ §6", "#777777", false));
    root.add_extra(ChatComponent::styled(
        format!("{total}"),
        &config.accent_color,
        false,
    ));

    // Separator (fine gray)
    root.add_extra(ChatComponent::styled(" §8│ ", "#555555", false));

    // Section 3: ETA (Label in cool gray, time in warm gold)
    let time_str = format_eta(eta_seconds);
    root.add_extra(ChatComponent::styled(
        format!("{eta_label}: "),
        "#aaaaaa",
        false,
    ));
    root.add_extra(ChatComponent::styled(
        format!("~{time_str}"),
        &config.accent_color,
        true,
    ));

    root
}

/// Builds the sleek, minimal BossBar title for Limbo / Holding mode:
/// `<gradient>ᴢᴇɴɪᴛʜ</gradient> §8│ §7Online: §x...#1`
#[must_use]
pub fn build_limbo_bossbar_title(config: &DisplayConfig, online_count: usize) -> ChatComponent {
    let zenith_text = if config.small_caps {
        to_small_caps("Zenith")
    } else {
        "Zenith".to_string()
    };
    let online_label = if config.small_caps {
        to_small_caps("Online")
    } else {
        "Online".to_string()
    };

    let mut root = ChatComponent::plain("");
    for comp in gradient_components(
        &zenith_text,
        &config.gradient_start,
        &config.gradient_end,
        true,
    ) {
        root.add_extra(comp);
    }

    root.add_extra(ChatComponent::styled(" §8│ ", "#555555", false));
    root.add_extra(ChatComponent::styled(
        format!("{online_label}: "),
        "#aaaaaa",
        false,
    ));
    root.add_extra(ChatComponent::styled(
        format!("#{online_count}"),
        &config.accent_color,
        true,
    ));

    root
}

/// Builds the Actionbar text for Limbo / Holding mode.
#[must_use]
pub fn build_limbo_actionbar_component(config: &DisplayConfig) -> ChatComponent {
    let holding_label = if config.small_caps {
        to_small_caps("Holding")
    } else {
        "Holding".to_string()
    };

    let mut root = ChatComponent::plain("");
    for comp in gradient_components(
        &holding_label,
        &config.gradient_start,
        &config.gradient_end,
        true,
    ) {
        root.add_extra(comp);
    }
    root.add_extra(ChatComponent::styled(" §8│ §7Type ", "#555555", false));
    root.add_extra(ChatComponent::styled(
        "/transfer",
        &config.accent_color,
        true,
    ));
    root.add_extra(ChatComponent::styled(" §7to connect", "#aaaaaa", false));
    root
}

/// Builds an actionbar status component (`Position: #X / Y │ Wait: ~mm:ss`).
#[must_use]
pub fn build_actionbar_component(
    config: &DisplayConfig,
    position: usize,
    total: usize,
    eta_seconds: u64,
) -> ChatComponent {
    let pos_label = if config.small_caps {
        to_small_caps(&config.position_label)
    } else {
        config.position_label.clone()
    };
    let eta_label = if config.small_caps {
        to_small_caps(&config.eta_label)
    } else {
        config.eta_label.clone()
    };

    let mut root = ChatComponent::plain("");
    root.add_extra(ChatComponent::styled(
        format!("{pos_label}: "),
        "#aaaaaa",
        false,
    ));
    root.add_extra(ChatComponent::styled(
        format!("#{position}"),
        &config.gradient_end,
        true,
    ));
    root.add_extra(ChatComponent::styled(
        format!(" §8/ §6{total} §8│ "),
        "#777777",
        false,
    ));
    root.add_extra(ChatComponent::styled(
        format!("{eta_label}: "),
        "#aaaaaa",
        false,
    ));
    root.add_extra(ChatComponent::styled(
        format!("~{}", format_eta(eta_seconds)),
        &config.accent_color,
        true,
    ));

    root
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_component_json_serialization() {
        let comp = ChatComponent::styled("TEST", "#ff3700", true);
        let json = comp.to_json();
        assert!(json.contains("\"text\":\"TEST\""));
        assert!(json.contains("\"color\":\"#ff3700\""));
        assert!(json.contains("\"bold\":true"));
    }

    #[test]
    fn test_component_to_legacy_string_hex() {
        let comp = ChatComponent::styled("TEST", "#ffaa00", true);
        let legacy = comp.to_legacy_string();
        assert_eq!(legacy, "§x§f§f§a§a§0§0§lTEST");
    }

    #[test]
    fn test_bossbar_title_building() {
        let config = DisplayConfig::default();
        let title = build_bossbar_title(&config, "QUEUE", 1, 10, 30);
        let legacy = title.to_legacy_string();
        assert!(legacy.contains("§x"));
        assert!(legacy.contains("#1"));
    }

    #[test]
    fn test_limbo_bossbar_title() {
        let config = DisplayConfig::default();
        let title = build_limbo_bossbar_title(&config, 5);
        let legacy = title.to_legacy_string();
        assert!(legacy.contains("§x"));
        assert!(legacy.contains("#5"));
    }
}
