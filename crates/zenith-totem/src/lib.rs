//! Crash evacuation lifeline, health poller, automatic backend re-routing,
//! and Horizon Native Turbo Fast-Path (`ctl.sock` IPC).

pub mod health;
pub mod lifeline;
pub mod prober;
pub mod turbo;

pub use health::{BackendHealth, ProbeReport, TotemConfig};
pub use lifeline::{LifelineHandles, TotemEvent, TotemLifeline, TotemStateMachine};
pub use prober::BackendProber;
pub use turbo::{HorizonTurboClient, HorizonTurboStatus, TurboEndpoint, TurboError};

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
