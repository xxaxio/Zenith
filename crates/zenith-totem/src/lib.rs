//! Crash evacuation lifeline, health poller, and automatic backend re-routing.

pub mod health;
pub mod lifeline;
pub mod prober;

pub use health::{BackendHealth, ProbeReport, TotemConfig};
pub use lifeline::{LifelineHandles, TotemEvent, TotemLifeline, TotemStateMachine};
pub use prober::BackendProber;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_totem_public_exports() {
        let config = TotemConfig::default();
        let (_lifeline, handles) = TotemLifeline::new("127.0.0.1:25566", config);
        assert!(!handles.health_rx.borrow().is_available());
    }
}
