//! Synthetic testkit, bot actors (Vanilla, Horizon, Adversary) and mock backends for Zenith integration testing.

pub mod backend;
pub mod client;
pub mod mutate;
pub mod proxy;

pub use backend::{MockBackendStats, MockHorizonBackend};
pub use client::{AdversaryStrategy, ClientError, ClientOutcome, ClientType, SyntheticClient};
pub use mutate::{MutationKind, MutatorPrng, mutate_packet};
pub use proxy::{HttpProxy, ProxyError, load_proxy_pool};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_type_creation() {
        let vanilla = SyntheticClient::new(
            "VanillaBot_1",
            ClientType::Vanilla {
                ping_jitter: std::time::Duration::from_millis(50),
            },
        );
        assert_eq!(vanilla.username, "VanillaBot_1");

        let adversary = SyntheticClient::new(
            "Attacker_1",
            ClientType::Adversary(AdversaryStrategy::MalformedVarInt),
        );
        assert_eq!(adversary.username, "Attacker_1");
    }
}
