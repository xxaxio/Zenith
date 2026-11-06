//! Display and HUD configuration structures.

use serde::{Deserialize, Serialize};

/// BossBar color variants supported by Protocol 777.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BossBarColor {
    Pink = 0,
    Blue = 1,
    Red = 2,
    Green = 3,
    Yellow = 4,
    Purple = 5,
    White = 6,
}

/// BossBar division overlay variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BossBarDivision {
    None = 0,
    Notches6 = 1,
    Notches10 = 2,
    Notches12 = 3,
    Notches20 = 4,
}

/// Complete configuration for HUD rendering, typography, and audio cues.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayConfig {
    /// Enable small-caps formatting for labels (e.g. "QUEUE" -> "Qᴜᴇᴜᴇ")
    pub small_caps: bool,

    /// Queue header title (shown when queue is active)
    pub queue_title: String,

    /// Header shown when queue is paused
    pub paused_title: String,

    /// Header shown when backend server is starting or offline
    pub starting_title: String,

    /// Header shown when backend server is under heavy load (throttled)
    pub throttled_title: String,

    /// Message shown during transfer dispatch
    pub transfer_title: String,

    /// Label prefix for queue position
    pub position_label: String,

    /// Label prefix for estimated wait time
    pub eta_label: String,

    /// Color when queue is flowing normally
    pub active_color: BossBarColor,

    /// Color when queue is paused or throttled
    pub paused_color: BossBarColor,

    /// Color when backend is starting or offline
    pub offline_color: BossBarColor,

    /// BossBar division overlay
    pub division: BossBarDivision,

    /// Enable sound effects
    pub enable_sounds: bool,

    /// Sound played when player advances forward in queue (e.g. pos < old_pos)
    pub click_sound_id: String,
    pub click_sound_pitch: f32,
    pub click_sound_volume: f32,

    /// Sound played when player is being transferred
    pub transfer_sound_id: String,
    pub transfer_sound_pitch: f32,
    pub transfer_sound_volume: f32,

    /// Optional background waiting sound/disc (empty string = none)
    pub waiting_music_id: String,

    /// Delay in milliseconds between visual cue and transfer packet (0 = instant new-gen)
    pub transfer_delay_ms: u64,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            small_caps: true,
            queue_title: "QUEUE".into(),
            paused_title: "QUEUE PAUSED".into(),
            starting_title: "SERVER STARTING".into(),
            throttled_title: "QUEUE THROTTLED".into(),
            transfer_title: "CONNECTING TO TARGET SERVER...".into(),
            position_label: "POSITION".into(),
            eta_label: "EST. WAIT".into(),
            active_color: BossBarColor::Blue,
            paused_color: BossBarColor::Purple,
            offline_color: BossBarColor::Red,
            division: BossBarDivision::None,
            enable_sounds: true,
            click_sound_id: "minecraft:ui.button.click".into(),
            click_sound_pitch: 1.5,
            click_sound_volume: 1.0,
            transfer_sound_id: "minecraft:ui.toast.challenge_complete".into(),
            transfer_sound_pitch: 1.0,
            transfer_sound_volume: 1.0,
            waiting_music_id: String::new(),
            transfer_delay_ms: 0,
        }
    }
}
