//! Minecraft Protocol 777 JSON Text Component serialization.

use crate::config::DisplayConfig;
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
    #[must_use]
    pub fn to_legacy_string(&self) -> String {
        let mut out = String::new();
        fn append_comp(c: &ChatComponent, out: &mut String) {
            if let Some(col) = &c.color {
                match col.as_str() {
                    "#00c6ff" | "cyan" | "aqua" => out.push_str("§b"),
                    "#f9d423" | "yellow" | "gold" => out.push_str("§e"),
                    "#00ff87" | "green" => out.push_str("§a"),
                    "#aaaaaa" | "gray" => out.push_str("§7"),
                    "#555555" | "#777777" | "dark_gray" => out.push_str("§8"),
                    _ => out.push_str("§f"),
                }
            }
            if c.bold == Some(true) {
                out.push_str("§l");
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

    // Section 1: Header / Title (Electric Blue #00c6ff)
    root.add_extra(ChatComponent::styled(title, "#00c6ff", true));

    // Separator
    root.add_extra(ChatComponent::styled(" §8• ", "#555555", false));

    // Section 2: Position (Label in Gray, numbers in Warm Gold #f9d423)
    root.add_extra(ChatComponent::styled(
        format!("{pos_label}: "),
        "#aaaaaa",
        false,
    ));
    root.add_extra(ChatComponent::styled(
        format!("#{position}"),
        "#f9d423",
        true,
    ));
    root.add_extra(ChatComponent::styled(" §8/ ", "#777777", false));
    root.add_extra(ChatComponent::styled(format!("{total}"), "#f9d423", false));

    // Separator
    root.add_extra(ChatComponent::styled(" §8│ ", "#555555", false));

    // Section 3: ETA (Label in Gray, time in Spring Neon Green #00ff87)
    let time_str = format_eta(eta_seconds);
    root.add_extra(ChatComponent::styled(
        format!("{eta_label}: "),
        "#aaaaaa",
        false,
    ));
    root.add_extra(ChatComponent::styled(
        format!("~{time_str}"),
        "#00ff87",
        true,
    ));

    root
}

/// Builds the formatted BossBar title for Limbo / Holding mode:
/// `§b§lᴢᴇɴɪᴛʜ §8• §7ᴠɪʀᴛᴜᴀʟ ʟɪᴍʙᴏ §8│ §7ᴏɴʟɪɴᴇ: §e§l#1 §8│ §7ʜᴇʟᴘ: §e/help`
#[must_use]
pub fn build_limbo_bossbar_title(config: &DisplayConfig, online_count: usize) -> ChatComponent {
    let zenith_text = if config.small_caps {
        to_small_caps("Zenith")
    } else {
        "Zenith".to_string()
    };
    let limbo_text = if config.small_caps {
        to_small_caps("Virtual Limbo")
    } else {
        "Virtual Limbo".to_string()
    };
    let online_label = if config.small_caps {
        to_small_caps("Online")
    } else {
        "Online".to_string()
    };
    let help_label = if config.small_caps {
        to_small_caps("Help")
    } else {
        "Help".to_string()
    };

    let mut root = ChatComponent::plain("");
    root.add_extra(ChatComponent::styled(zenith_text, "#00c6ff", true));
    root.add_extra(ChatComponent::styled(" §8• ", "#555555", false));
    root.add_extra(ChatComponent::styled(limbo_text, "#aaaaaa", false));
    root.add_extra(ChatComponent::styled(" §8│ ", "#555555", false));
    root.add_extra(ChatComponent::styled(
        format!("{online_label}: "),
        "#aaaaaa",
        false,
    ));
    root.add_extra(ChatComponent::styled(
        format!("#{online_count}"),
        "#f9d423",
        true,
    ));
    root.add_extra(ChatComponent::styled(" §8│ ", "#555555", false));
    root.add_extra(ChatComponent::styled(
        format!("{help_label}: "),
        "#aaaaaa",
        false,
    ));
    root.add_extra(ChatComponent::styled("/help", "#f9d423", false));

    root
}

/// Builds the Actionbar text for Limbo / Holding mode:
/// `§8[ §bᴢᴇɴɪᴛʜ §8] §7Connected to Holding Void §8• §7Type §b/transfer §7to join`
#[must_use]
pub fn build_limbo_actionbar_component(config: &DisplayConfig) -> ChatComponent {
    let zenith_text = if config.small_caps {
        to_small_caps("Zenith")
    } else {
        "Zenith".to_string()
    };
    let void_text = if config.small_caps {
        to_small_caps("Holding Void")
    } else {
        "Holding Void".to_string()
    };

    let mut root = ChatComponent::plain("");
    root.add_extra(ChatComponent::styled("§8[ ", "#555555", false));
    root.add_extra(ChatComponent::styled(zenith_text, "#00c6ff", true));
    root.add_extra(ChatComponent::styled(
        " §8] §7Connected to ",
        "#555555",
        false,
    ));
    root.add_extra(ChatComponent::styled(void_text, "#aaaaaa", false));
    root.add_extra(ChatComponent::styled(
        " §8• §7Type §b/transfer §7to join",
        "#aaaaaa",
        false,
    ));

    root
}

/// Builds an actionbar status component (`Position: #X / Y · ETA: ~mm:ss`).
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
        format!("{pos_label} "),
        "#aaaaaa",
        false,
    ));
    root.add_extra(ChatComponent::styled(
        format!("#{position}"),
        "#f9d423",
        true,
    ));
    root.add_extra(ChatComponent::styled(
        format!(" / {total}"),
        "#777777",
        false,
    ));
    root.add_extra(ChatComponent::styled(" · ", "#555555", false));
    root.add_extra(ChatComponent::styled(
        format!("{eta_label} ~"),
        "#aaaaaa",
        false,
    ));
    root.add_extra(ChatComponent::styled(
        format_eta(eta_seconds),
        "#00ff87",
        true,
    ));
    root
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_component_json_serialization() {
        let mut comp = ChatComponent::plain("");
        comp.add_extra(ChatComponent::styled("QUEUE", "#00c6ff", true));
        let json = comp.to_json();
        assert!(json.contains("#00c6ff"));
        assert!(json.contains("QUEUE"));
    }

    #[test]
    fn test_bossbar_title_generation() {
        let config = DisplayConfig::default();
        let comp = build_bossbar_title(&config, "QUEUE", 5, 25, 45);
        let json = comp.to_json();
        assert!(json.contains("ǫᴜᴇᴜᴇ"));
        assert!(json.contains("#f9d423"));
        assert!(json.contains("00:45"));
    }
}
