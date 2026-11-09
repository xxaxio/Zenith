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
    /// Maps RGB hex colors to high-contrast, universally supported Minecraft 16-color codes
    /// to avoid client-side parsing bugs (where `§x` defaults to black or lime green).
    #[must_use]
    pub fn to_legacy_string(&self) -> String {
        let mut out = String::new();
        fn append_comp(c: &ChatComponent, out: &mut String) {
            if let Some(col) = &c.color {
                if let Some(rgb) = RgbColor::from_hex(col) {
                    out.push_str(rgb.to_vanilla_code());
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

    // Separator (fine steel gray)
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
    root.add_extra(ChatComponent::styled(" §8/ §7", "#777777", false));
    root.add_extra(ChatComponent::styled(format!("{total}"), "#aaaaaa", false));

    // Separator (fine steel gray)
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
/// `ᴢᴇɴɪᴛʜ §8│ §7Virtual Holding §8│ §e#1 Players`
#[must_use]
pub fn build_limbo_bossbar_title(config: &DisplayConfig, online_count: usize) -> ChatComponent {
    let zenith_text = if config.small_caps {
        to_small_caps("Zenith")
    } else {
        "Zenith".to_string()
    };
    let holding_label = if config.small_caps {
        to_small_caps("Virtual Holding")
    } else {
        "Virtual Holding".to_string()
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
        format!("{holding_label} §8│ "),
        "#aaaaaa",
        false,
    ));
    root.add_extra(ChatComponent::styled(
        format!("#{online_count} "),
        &config.accent_color,
        true,
    ));
    root.add_extra(ChatComponent::styled(
        if online_count == 1 {
            "Player"
        } else {
            "Players"
        },
        "#aaaaaa",
        false,
    ));

    root
}

/// Builds the Actionbar text for Limbo / Holding mode.
#[must_use]
pub fn build_limbo_actionbar_component(config: &DisplayConfig) -> ChatComponent {
    let mut root = ChatComponent::plain("");
    root.add_extra(ChatComponent::styled("§e§l● ", &config.accent_color, true));
    root.add_extra(ChatComponent::styled(
        "Virtual Holding Dimension §8│ §7Type ",
        "#aaaaaa",
        false,
    ));
    root.add_extra(ChatComponent::styled(
        "/transfer",
        &config.accent_color,
        true,
    ));
    root.add_extra(ChatComponent::styled(" §7to connect", "#aaaaaa", false));
    root
}

/// Builds an actionbar status component (`● Queue: #1 (1 in line) │ Est: ~mm:ss`).
#[must_use]
pub fn build_actionbar_component(
    config: &DisplayConfig,
    position: usize,
    total: usize,
    eta_seconds: u64,
) -> ChatComponent {
    let mut root = ChatComponent::plain("");
    root.add_extra(ChatComponent::styled("§6§l● ", &config.gradient_end, true));
    root.add_extra(ChatComponent::styled("Queue: ", "#aaaaaa", false));
    root.add_extra(ChatComponent::styled(
        format!("#{position}"),
        &config.gradient_end,
        true,
    ));
    root.add_extra(ChatComponent::styled(
        format!(" §8(§7{total} in line§8)"),
        "#777777",
        false,
    ));
    root.add_extra(ChatComponent::styled(" §8│ ", "#555555", false));

    let time_str = format_eta(eta_seconds);
    root.add_extra(ChatComponent::styled("Est: ", "#aaaaaa", false));
    root.add_extra(ChatComponent::styled(
        format!("~{time_str}"),
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
        assert_eq!(legacy, "§6§lTEST");
    }

    #[test]
    fn test_bossbar_title_building() {
        let config = DisplayConfig::default();
        let title = build_bossbar_title(&config, "ZENITH", 1, 10, 30);
        let legacy = title.to_legacy_string();
        assert!(legacy.contains("#1"));
        assert!(legacy.contains("│"));
    }

    #[test]
    fn test_limbo_bossbar_title() {
        let config = DisplayConfig::default();
        let title = build_limbo_bossbar_title(&config, 5);
        let legacy = title.to_legacy_string();
        assert!(legacy.contains("#5"));
        assert!(legacy.contains("│"));
    }
}
