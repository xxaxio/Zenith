//! Dynamic HUD interface builders: BossBar, Actionbar, Titles, Sound cues, and Transfer packets.

pub mod actionbar;
pub mod bossbar;
pub mod component;
pub mod config;
pub mod sound;
pub mod title;
pub mod transfer;
pub mod typography;

pub use actionbar::{encode_actionbar_packet, encode_system_chat_packet};
pub use bossbar::{
    encode_bossbar_add, encode_bossbar_health, encode_bossbar_remove, encode_bossbar_style,
    encode_bossbar_title,
};
pub use component::{ChatComponent, build_actionbar_component, build_bossbar_title};
pub use config::{BossBarColor, BossBarDivision, DisplayConfig};
pub use sound::{SoundCategory, encode_click_sound, encode_sound_packet, encode_transfer_sound};
pub use title::{
    encode_clear_titles_packet, encode_subtitle_packet, encode_title_packet,
    encode_title_times_packet,
};
pub use transfer::encode_transfer_packet;
pub use typography::{format_eta, to_small_caps};

/// High-level manager coordinating HUD display rendering for queue sessions.
#[derive(Debug, Clone)]
pub struct HudRenderer {
    config: DisplayConfig,
}

impl HudRenderer {
    /// Creates a new HUD renderer with the provided configuration.
    #[must_use]
    pub fn new(config: DisplayConfig) -> Self {
        Self { config }
    }

    /// Access the underlying display configuration.
    #[must_use]
    pub fn config(&self) -> &DisplayConfig {
        &self.config
    }

    /// Renders an initial BossBar Add packet for a queued session.
    #[must_use]
    pub fn render_bossbar_add(
        &self,
        uuid: u128,
        position: usize,
        total: usize,
        eta_seconds: u64,
    ) -> Vec<u8> {
        let title_comp = build_bossbar_title(
            &self.config,
            &self.config.queue_title,
            position,
            total,
            eta_seconds,
        );
        let progress = calculate_progress(position, total);
        encode_bossbar_add(
            uuid,
            &title_comp.to_legacy_string(),
            progress,
            self.config.active_color,
            self.config.division,
        )
    }

    /// Renders BossBar updates (title and progress) when position changes.
    #[must_use]
    pub fn render_bossbar_update(
        &self,
        uuid: u128,
        position: usize,
        total: usize,
        eta_seconds: u64,
    ) -> (Vec<u8>, Vec<u8>) {
        let title_comp = build_bossbar_title(
            &self.config,
            &self.config.queue_title,
            position,
            total,
            eta_seconds,
        );
        let progress = calculate_progress(position, total);
        let title_pkt = encode_bossbar_title(uuid, &title_comp.to_legacy_string());
        let health_pkt = encode_bossbar_health(uuid, progress);
        (title_pkt, health_pkt)
    }

    /// Renders a BossBar Remove packet when a player leaves the queue.
    #[must_use]
    pub fn render_bossbar_remove(&self, uuid: u128) -> Vec<u8> {
        encode_bossbar_remove(uuid)
    }

    /// Renders an Actionbar text packet with position and ETA.
    #[must_use]
    pub fn render_actionbar(&self, position: usize, total: usize, eta_seconds: u64) -> Vec<u8> {
        let comp = build_actionbar_component(&self.config, position, total, eta_seconds);
        encode_actionbar_packet(&comp.to_legacy_string())
    }

    /// Renders a UI click sound packet if sounds are enabled.
    #[must_use]
    pub fn render_click_sound(&self) -> Option<Vec<u8>> {
        if self.config.enable_sounds {
            Some(encode_click_sound(self.config.click_sound_pitch))
        } else {
            None
        }
    }

    /// Renders the transfer sequence: optional transfer sound and the clientbound Transfer packet.
    #[must_use]
    pub fn render_transfer(&self, host: &str, port: u16) -> (Option<Vec<u8>>, Vec<u8>) {
        let sound = if self.config.enable_sounds {
            Some(encode_transfer_sound(self.config.transfer_sound_pitch))
        } else {
            None
        };
        let transfer_pkt = encode_transfer_packet(host, port);
        (sound, transfer_pkt)
    }
}

impl Default for HudRenderer {
    fn default() -> Self {
        Self::new(DisplayConfig::default())
    }
}

/// Calculates the normalized 0.0 to 1.0 progress for BossBar display.
#[must_use]
pub fn calculate_progress(position: usize, total: usize) -> f32 {
    if total == 0 {
        return 1.0;
    }
    if position <= 1 {
        return 1.0;
    }
    // Progress starts near 0.0 when far back and rises to 1.0 as position reaches 1
    let ratio = (total.saturating_sub(position)) as f32 / total as f32;
    ratio.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hud_renderer_lifecycle() {
        let renderer = HudRenderer::default();
        let uuid = 0xAAAA_BBBB_CCCC_DDDD_EEEE_FFFF_0000_1111;

        let add_pkt = renderer.render_bossbar_add(uuid, 10, 100, 30);
        assert!(!add_pkt.is_empty());

        let (title_pkt, health_pkt) = renderer.render_bossbar_update(uuid, 9, 100, 27);
        assert!(!title_pkt.is_empty());
        assert!(!health_pkt.is_empty());

        let click = renderer.render_click_sound();
        assert!(click.is_some());

        let (sound, xfer) = renderer.render_transfer("play.horizon.dev", 25565);
        assert!(sound.is_some());
        assert!(!xfer.is_empty());

        let remove_pkt = renderer.render_bossbar_remove(uuid);
        assert!(!remove_pkt.is_empty());
    }

    #[test]
    fn test_calculate_progress() {
        assert_eq!(calculate_progress(1, 10), 1.0);
        assert_eq!(calculate_progress(10, 10), 0.0);
        assert_eq!(calculate_progress(5, 10), 0.5);
    }
}
