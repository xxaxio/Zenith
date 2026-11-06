//! HUD interface builders: dynamic BossBars, Actionbar status, and audio cues.

/// BossBar color variants supported by Protocol 777.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BossBarColor {
    Pink = 0,
    Blue = 1,
    Red = 2,
    Green = 3,
    Yellow = 4,
    Purple = 5,
    White = 6,
}

/// Formats the queue progress display string.
#[must_use]
pub fn format_queue_status(position: usize, total: usize, eta_seconds: u32) -> String {
    format!("Queue: #{position} / {total} (Est. wait: {eta_seconds}s)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_queue_status() {
        let text = format_queue_status(5, 100, 20);
        assert_eq!(text, "Queue: #5 / 100 (Est. wait: 20s)");
    }
}
