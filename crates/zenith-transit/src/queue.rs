//! High-performance indexed transit queue with priority tiers, ping sorting, and wave draining.

use std::collections::HashMap;
use std::time::Instant;
use zenith_session::SessionId;

use crate::config::{SortingPolicy, TransitConfig};
use crate::entry::QueueEntry;
use crate::health::TargetServerHealth;
use crate::priority::QueuePriority;
use crate::wave::WaveScheduler;

/// High-performance indexed FIFO / Ping transit queue manager.
#[derive(Debug)]
pub struct TransitQueue {
    config: TransitConfig,
    entries: HashMap<SessionId, QueueEntry>,
    paused: bool,
    scheduler: WaveScheduler,
}

impl TransitQueue {
    /// Creates a new transit queue with the specified configuration.
    #[must_use]
    pub fn new(config: TransitConfig) -> Self {
        Self {
            config,
            entries: HashMap::new(),
            paused: false,
            scheduler: WaveScheduler::new(),
        }
    }

    /// Access the underlying configuration.
    #[must_use]
    pub fn config(&self) -> &TransitConfig {
        &self.config
    }

    /// Mutably update configuration (e.g. on reload).
    pub fn set_config(&mut self, config: TransitConfig) {
        self.config = config;
    }

    /// Adds a player entry into the transit queue.
    pub fn enqueue(&mut self, entry: QueueEntry) {
        self.entries.insert(entry.session_id, entry);
    }

    /// Updates the priority of an active entry in the queue.
    pub fn update_priority(&mut self, session_id: SessionId, priority: QueuePriority) {
        if let Some(entry) = self.entries.get_mut(&session_id) {
            entry.priority = priority;
        }
    }

    /// Updates the measured ping latency of an active entry in the queue.
    pub fn update_ping(&mut self, session_id: SessionId, ping_ms: u32) {
        if let Some(entry) = self.entries.get_mut(&session_id) {
            entry.ping_ms = ping_ms;
        }
    }

    /// Returns the measured ping latency for a session, if queued.
    #[must_use]
    pub fn ping_of(&self, session_id: SessionId) -> Option<u32> {
        self.entries.get(&session_id).map(|e| e.ping_ms)
    }

    /// Finds a session ID by player username (case-insensitive).
    #[must_use]
    pub fn session_id_of(&self, username: &str) -> Option<SessionId> {
        let lower = username.to_lowercase();
        self.entries
            .values()
            .find(|e| e.username.to_lowercase() == lower)
            .map(|e| e.session_id)
    }

    /// Removes a player from the queue in O(1) upon disconnect or cancellation.
    pub fn remove(&mut self, session_id: SessionId) -> Option<QueueEntry> {
        self.entries.remove(&session_id)
    }

    /// Returns a sorted vector of references to queued entries according to tiers and sorting policy.
    #[must_use]
    pub fn sorted_entries(&self) -> Vec<QueueEntry> {
        let mut list: Vec<QueueEntry> = self.entries.values().cloned().collect();
        let policy = self.config.sorting_policy;

        list.sort_by(|a, b| {
            // Primary order: Priority tier descending (Staff > VIP > Standard)
            let prio_cmp = b.priority.cmp(&a.priority);
            if prio_cmp != std::cmp::Ordering::Equal {
                return prio_cmp;
            }

            // Secondary order: based on sorting policy within the same tier
            match policy {
                SortingPolicy::Fifo => a.enqueued_at.cmp(&b.enqueued_at),
                SortingPolicy::PingAscending => a
                    .ping_ms
                    .cmp(&b.ping_ms)
                    .then_with(|| a.enqueued_at.cmp(&b.enqueued_at)),
                SortingPolicy::PingDescending => b
                    .ping_ms
                    .cmp(&a.ping_ms)
                    .then_with(|| a.enqueued_at.cmp(&b.enqueued_at)),
            }
        });

        list
    }

    /// Returns the 1-based position and total count for a given session.
    #[must_use]
    pub fn position_of(&self, session_id: SessionId) -> Option<(usize, usize)> {
        if !self.entries.contains_key(&session_id) {
            return None;
        }

        let sorted = self.sorted_entries();
        let total = sorted.len();
        sorted
            .iter()
            .position(|e| e.session_id == session_id)
            .map(|idx| (idx + 1, total))
    }

    /// Computes the 1-based ranked positions for all queued players in a single O(N log N) pass.
    /// Returns a map of `session_id -> (position_1_based, total_queued)`.
    #[must_use]
    pub fn ranked_positions(&self) -> std::collections::HashMap<SessionId, (usize, usize)> {
        let sorted = self.sorted_entries();
        let total = sorted.len();
        sorted
            .into_iter()
            .enumerate()
            .map(|(idx, entry)| (entry.session_id, (idx + 1, total)))
            .collect()
    }

    /// Evaluates and drains the next wave of eligible players if the schedule interval is due.
    pub fn drain_wave(&mut self, now: Instant, health: &TargetServerHealth) -> Vec<QueueEntry> {
        if self.paused || !self.scheduler.is_due(now, health, &self.config) {
            return Vec::new();
        }

        let wave_size = self.scheduler.current_wave_size(health, &self.config);
        if wave_size == 0 {
            return Vec::new();
        }

        let sorted = self.sorted_entries();
        let to_drain: Vec<QueueEntry> = sorted.into_iter().take(wave_size).collect();

        for entry in &to_drain {
            self.entries.remove(&entry.session_id);
        }

        if !to_drain.is_empty() {
            self.scheduler.record_drain(now);
        }

        to_drain
    }

    /// Sets the queue pause state explicitly.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// Toggles the queue pause state (e.g. for maintenance or admin `/queue` command).
    pub fn toggle_pause(&mut self) -> bool {
        self.paused = !self.paused;
        self.paused
    }

    /// Checks if the queue is currently paused.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Returns the total number of waiting players.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Checks whether the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for TransitQueue {
    fn default() -> Self {
        Self::new(TransitConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::priority::QueuePriority;

    #[test]
    fn test_tier_and_ping_sorting() {
        let config = TransitConfig {
            sorting_policy: SortingPolicy::PingAscending,
            ..Default::default()
        };
        let mut queue = TransitQueue::new(config);

        let s_std_fast = SessionId(1);
        let s_std_slow = SessionId(2);
        let s_vip = SessionId(3);
        let s_staff = SessionId(4);

        queue.enqueue(QueueEntry::new(
            s_std_slow,
            2,
            "std_slow",
            QueuePriority::Standard,
            80,
            "hub",
        ));
        queue.enqueue(QueueEntry::new(
            s_std_fast,
            1,
            "std_fast",
            QueuePriority::Standard,
            15,
            "hub",
        ));
        queue.enqueue(QueueEntry::new(
            s_vip,
            3,
            "vip_player",
            QueuePriority::Vip,
            50,
            "hub",
        ));
        queue.enqueue(QueueEntry::new(
            s_staff,
            4,
            "admin",
            QueuePriority::Staff,
            120,
            "hub",
        ));

        let sorted = queue.sorted_entries();
        // 1. Staff comes first (weight 100)
        assert_eq!(sorted[0].session_id, s_staff);
        // 2. VIP comes next (weight 10)
        assert_eq!(sorted[1].session_id, s_vip);
        // 3. Between standards, s_std_fast (15ms) comes before s_std_slow (80ms)
        assert_eq!(sorted[2].session_id, s_std_fast);
        assert_eq!(sorted[3].session_id, s_std_slow);

        // Position check
        assert_eq!(queue.position_of(s_staff), Some((1, 4)));
        assert_eq!(queue.position_of(s_std_slow), Some((4, 4)));
    }

    #[test]
    fn test_wave_drain() {
        let config = TransitConfig::default();
        let mut queue = TransitQueue::new(config);

        for i in 1..=15 {
            queue.enqueue(QueueEntry::new(
                SessionId(i),
                i as u128,
                format!("player_{i}"),
                QueuePriority::Standard,
                20,
                "survival",
            ));
        }

        assert_eq!(queue.len(), 15);
        let health = TargetServerHealth::default();
        let now = Instant::now();

        // Nominal wave drains 10 players
        let wave = queue.drain_wave(now, &health);
        assert_eq!(wave.len(), 10);
        assert_eq!(queue.len(), 5);

        // Immediately afterwards, wave is not due
        let wave2 = queue.drain_wave(now, &health);
        assert_eq!(wave2.len(), 0);
    }
}
