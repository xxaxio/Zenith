//! Crash evacuation lifeline, health state machine, and auto-recovery event broadcaster.

use std::time::Duration;

use tokio::sync::{broadcast, watch};
use tracing::{error, info, warn};

use crate::health::{BackendHealth, ProbeReport, TotemConfig};
use crate::prober::BackendProber;

/// Events broadcast by the Totem lifeline when backend status shifts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TotemEvent {
    /// Backend transitioned from healthy/degraded to offline after threshold failures.
    BackendDown {
        consecutive_failures: u32,
        error: String,
    },
    /// Backend transitioned from offline back to available after threshold successes.
    BackendRecovered {
        latency_ms: u32,
        online_players: Option<u32>,
    },
    /// Periodic status update (e.g. latency fluctuation, degraded state, or recovery progress).
    StatusUpdate {
        health: BackendHealth,
        latency_ms: u32,
    },
}

/// Deterministic state machine tracking backend health transitions.
#[derive(Debug, Clone)]
pub struct TotemStateMachine {
    config: TotemConfig,
    current_health: BackendHealth,
    consecutive_failures: u32,
    consecutive_successes: u32,
}

impl TotemStateMachine {
    /// Creates a new state machine initialized in Offline state.
    #[must_use]
    pub fn new(config: TotemConfig) -> Self {
        Self {
            config,
            current_health: BackendHealth::Offline {
                consecutive_failures: 0,
                last_error: "Initializing".into(),
            },
            consecutive_failures: 0,
            consecutive_successes: 0,
        }
    }

    /// Returns the current evaluated health.
    #[must_use]
    pub fn current_health(&self) -> &BackendHealth {
        &self.current_health
    }

    /// Evaluates a new probe report and transitions internal state.
    /// Returns `Some(TotemEvent)` if an important threshold or state change occurred.
    pub fn process_report(&mut self, report: &ProbeReport) -> Option<TotemEvent> {
        if report.is_online {
            self.consecutive_successes = self.consecutive_successes.saturating_add(1);
            self.consecutive_failures = 0;

            let latency = report.latency_ms;
            let was_offline = matches!(self.current_health, BackendHealth::Offline { .. });

            // Determine healthy vs degraded
            let new_health = if latency > self.config.degraded_latency_ms {
                BackendHealth::Degraded {
                    latency_ms: latency,
                    reason: format!(
                        "High ping ({}ms > {}ms threshold)",
                        latency, self.config.degraded_latency_ms
                    ),
                }
            } else {
                BackendHealth::Healthy {
                    latency_ms: latency,
                }
            };

            if was_offline {
                if self.consecutive_successes >= self.config.min_successes_for_recovery {
                    self.current_health = new_health;
                    return Some(TotemEvent::BackendRecovered {
                        latency_ms: latency,
                        online_players: report.online_players,
                    });
                }
                // Still waiting for confirmation successes before declaring recovery
                None
            } else {
                let health_changed = self.current_health != new_health;
                self.current_health = new_health;

                if health_changed {
                    Some(TotemEvent::StatusUpdate {
                        health: self.current_health.clone(),
                        latency_ms: latency,
                    })
                } else {
                    None
                }
            }
        } else {
            self.consecutive_failures = self.consecutive_failures.saturating_add(1);
            self.consecutive_successes = 0;

            let err = report
                .error
                .clone()
                .unwrap_or_else(|| "Unknown probe failure".into());

            let was_available = self.current_health.is_available();

            if was_available && self.consecutive_failures >= self.config.min_failures_for_offline {
                self.current_health = BackendHealth::Offline {
                    consecutive_failures: self.consecutive_failures,
                    last_error: err.clone(),
                };
                Some(TotemEvent::BackendDown {
                    consecutive_failures: self.consecutive_failures,
                    error: err,
                })
            } else {
                if !was_available {
                    // Update failure count in offline state
                    self.current_health = BackendHealth::Offline {
                        consecutive_failures: self.consecutive_failures,
                        last_error: err,
                    };
                }
                None
            }
        }
    }
}

/// The Totem Lifeline runner supervising periodic probes and emitting events.
pub struct TotemLifeline {
    prober: BackendProber,
    config: TotemConfig,
    state: TotemStateMachine,
    event_tx: broadcast::Sender<TotemEvent>,
    health_tx: watch::Sender<BackendHealth>,
}

impl TotemLifeline {
    /// Initializes a new lifeline for the specified backend address.
    pub fn new(target_addr: impl Into<String>, config: TotemConfig) -> (Self, LifelineHandles) {
        let prober =
            BackendProber::new(target_addr, Duration::from_millis(config.probe_timeout_ms));
        let state = TotemStateMachine::new(config.clone());
        let initial_health = state.current_health().clone();

        let (event_tx, event_rx) = broadcast::channel(64);
        let (health_tx, health_rx) = watch::channel(initial_health);

        let lifeline = Self {
            prober,
            config,
            state,
            event_tx,
            health_tx,
        };

        let handles = LifelineHandles {
            event_rx,
            health_rx,
        };

        (lifeline, handles)
    }

    /// Runs the lifeline probing loop indefinitely until stopped.
    pub async fn run(mut self) {
        let interval = Duration::from_millis(self.config.probe_interval_ms);
        let mut timer = tokio::time::interval(interval);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        info!(
            target: "zenith_totem",
            target_addr = %self.prober.target_addr,
            probe_interval_ms = self.config.probe_interval_ms,
            "Totem Lifeline active and monitoring target backend"
        );

        loop {
            timer.tick().await;

            let report = self.prober.probe().await;
            if let Some(event) = self.state.process_report(&report) {
                match &event {
                    TotemEvent::BackendDown {
                        consecutive_failures,
                        error,
                    } => {
                        error!(
                            target: "zenith_totem",
                            target_addr = %self.prober.target_addr,
                            consecutive_failures,
                            error = %error,
                            "TOTEM ALERT: Target backend is DOWN! Pausing transfers, securing limbo"
                        );
                    }
                    TotemEvent::BackendRecovered {
                        latency_ms,
                        online_players,
                    } => {
                        info!(
                            target: "zenith_totem",
                            target_addr = %self.prober.target_addr,
                            latency_ms,
                            online_players = ?online_players,
                            "TOTEM ALERT: Target backend has RECOVERED! Resuming transfer drain"
                        );
                    }
                    TotemEvent::StatusUpdate { health, latency_ms } => {
                        warn!(
                            target: "zenith_totem",
                            target_addr = %self.prober.target_addr,
                            ?health,
                            latency_ms,
                            "Totem backend status update"
                        );
                    }
                }

                // Broadcast event
                let _ = self.event_tx.send(event);
            }

            // Always update watch channel with latest state
            let _ = self.health_tx.send(self.state.current_health().clone());
        }
    }
}

/// Receivers held by session handlers or queue controllers.
pub struct LifelineHandles {
    /// Broadcast stream for state transitions (`BackendDown`, `BackendRecovered`).
    pub event_rx: broadcast::Receiver<TotemEvent>,
    /// Continuous watch receiver reflecting the latest evaluated health.
    pub health_rx: watch::Receiver<BackendHealth>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_totem_state_transitions() {
        let config = TotemConfig {
            probe_interval_ms: 100,
            probe_timeout_ms: 50,
            degraded_latency_ms: 100,
            min_failures_for_offline: 2,
            min_successes_for_recovery: 2,
        };

        let mut sm = TotemStateMachine::new(config);
        assert!(!sm.current_health().is_available());

        // First success: not recovered yet (min_successes_for_recovery = 2)
        let rep1 =
            ProbeReport::success(20, Some(777), Some("26.3".into()), Some(0), Some(50), None);
        assert_eq!(sm.process_report(&rep1), None);
        assert!(!sm.current_health().is_available());

        // Second success: now recovered!
        let ev = sm.process_report(&rep1);
        assert_eq!(
            ev,
            Some(TotemEvent::BackendRecovered {
                latency_ms: 20,
                online_players: Some(0),
            })
        );
        assert!(sm.current_health().is_available());
        assert_eq!(
            sm.current_health(),
            &BackendHealth::Healthy { latency_ms: 20 }
        );

        // Latency spike: transition to degraded
        let rep_lag =
            ProbeReport::success(250, Some(777), Some("26.3".into()), Some(5), Some(50), None);
        let ev_deg = sm.process_report(&rep_lag);
        assert!(matches!(ev_deg, Some(TotemEvent::StatusUpdate { .. })));
        assert!(matches!(
            sm.current_health(),
            BackendHealth::Degraded { .. }
        ));

        // First failure: still considered available (min_failures_for_offline = 2)
        let rep_fail = ProbeReport::failure("Connection reset");
        assert_eq!(sm.process_report(&rep_fail), None);
        assert!(sm.current_health().is_available());

        // Second failure: declared BackendDown!
        let ev_down = sm.process_report(&rep_fail);
        assert_eq!(
            ev_down,
            Some(TotemEvent::BackendDown {
                consecutive_failures: 2,
                error: "Connection reset".into(),
            })
        );
        assert!(!sm.current_health().is_available());
    }
}
