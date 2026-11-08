//! Soft-drain backend evacuation manager for Zenith.
//!
//! Allows an operator to smoothly drain all players off a backend before a reload,
//! restart, or maintenance window without kicking anyone. Players are gently
//! redirected to an alternate backend or held safely in the Limbo void queue
//! until replacement capacity is ready.

use std::collections::HashMap;
use std::time::Instant;

/// Status and metadata of an active backend evacuation order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvacuationOrder {
    /// Identifier of the backend being evacuated (e.g. "hub-01", "survival-main").
    pub backend_id: String,
    /// Optional fallback destination for evacuated players (e.g. "hub-02", "limbo").
    pub fallback_backend: Option<String>,
    /// Wall-clock instant when evacuation was initiated.
    pub started_at: Instant,
    /// Number of players successfully drained/migrated off this backend.
    pub drained_count: usize,
}

/// Manages soft-drain evacuation state across all registered backends.
#[derive(Debug, Default)]
pub struct EvacuationManager {
    orders: HashMap<String, EvacuationOrder>,
}

impl EvacuationManager {
    /// Creates a new, empty evacuation manager.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Initiates a soft-drain evacuation for the specified backend.
    ///
    /// If an evacuation is already in progress for this backend, its fallback target is updated.
    pub fn start_evacuation(
        &mut self,
        backend_id: impl Into<String>,
        fallback_backend: Option<String>,
    ) {
        let id = backend_id.into();
        self.orders.insert(
            id.clone(),
            EvacuationOrder {
                backend_id: id,
                fallback_backend,
                started_at: Instant::now(),
                drained_count: 0,
            },
        );
    }

    /// Returns `true` if the specified backend is currently marked for evacuation.
    #[must_use]
    pub fn is_evacuating(&self, backend_id: &str) -> bool {
        self.orders.contains_key(backend_id)
    }

    /// Resolves the effective routing target for a player wanting to join `target_backend`.
    ///
    /// If the backend is under evacuation, returns its configured fallback destination,
    /// or `None` if players should be held in Limbo.
    #[must_use]
    pub fn resolve_target<'a>(&'a self, target_backend: &'a str) -> Option<&'a str> {
        if let Some(order) = self.orders.get(target_backend) {
            order.fallback_backend.as_deref()
        } else {
            Some(target_backend)
        }
    }

    /// Records that a player has successfully migrated off an evacuating backend.
    pub fn record_evacuated_player(&mut self, backend_id: &str) {
        if let Some(order) = self.orders.get_mut(backend_id) {
            order.drained_count += 1;
        }
    }

    /// Cancels an active evacuation order, restoring normal ingress traffic to the backend.
    pub fn cancel_evacuation(&mut self, backend_id: &str) -> bool {
        self.orders.remove(backend_id).is_some()
    }

    /// Returns a list of all currently active evacuation orders.
    #[must_use]
    pub fn active_evacuations(&self) -> Vec<&EvacuationOrder> {
        self.orders.values().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_evacuation_lifecycle() {
        let mut mgr = EvacuationManager::new();
        assert!(!mgr.is_evacuating("server-1"));
        assert_eq!(mgr.resolve_target("server-1"), Some("server-1"));

        // Start evacuation with fallback to hub-02
        mgr.start_evacuation("server-1", Some("hub-02".into()));
        assert!(mgr.is_evacuating("server-1"));
        assert_eq!(mgr.resolve_target("server-1"), Some("hub-02"));

        // Record drained players
        mgr.record_evacuated_player("server-1");
        mgr.record_evacuated_player("server-1");

        let active = mgr.active_evacuations();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].drained_count, 2);

        // Cancel evacuation
        assert!(mgr.cancel_evacuation("server-1"));
        assert!(!mgr.is_evacuating("server-1"));
        assert_eq!(mgr.resolve_target("server-1"), Some("server-1"));
    }

    #[test]
    fn test_evacuation_hold_in_limbo() {
        let mut mgr = EvacuationManager::new();
        // Start evacuation with no fallback destination (hold in Limbo)
        mgr.start_evacuation("survival-01", None);
        assert!(mgr.is_evacuating("survival-01"));
        assert_eq!(mgr.resolve_target("survival-01"), None);
    }
}
