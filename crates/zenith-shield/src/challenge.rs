//! Invisible zero-UI protocol challenge for anti-bot verification.

use std::time::{Duration, Instant};

/// Status of an active protocol challenge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChallengeStatus {
    /// Challenge packet emitted, awaiting echo from genuine Minecraft client.
    Pending,
    /// Challenge successfully verified and completed.
    Passed,
    /// Challenge timed out or client replied with invalid token.
    Failed,
}

/// A zero-UI protocol challenge tracking client legitimacy.
#[derive(Debug, Clone)]
pub struct ProtocolChallenge {
    challenge_id: u64,
    created_at: Instant,
    timeout: Duration,
    status: ChallengeStatus,
}

impl ProtocolChallenge {
    pub fn new(challenge_id: u64, timeout: Duration) -> Self {
        Self {
            challenge_id,
            created_at: Instant::now(),
            timeout,
            status: ChallengeStatus::Pending,
        }
    }

    /// Challenge token expected from client.
    pub fn challenge_id(&self) -> u64 {
        self.challenge_id
    }

    /// Current status of the challenge.
    pub fn status(&self) -> ChallengeStatus {
        self.status
    }

    /// Verifies an incoming echo response against the expected token.
    pub fn verify_response(&mut self, received_id: u64, now: Instant) -> bool {
        if self.status != ChallengeStatus::Pending {
            return self.status == ChallengeStatus::Passed;
        }

        if now.duration_since(self.created_at) > self.timeout {
            self.status = ChallengeStatus::Failed;
            return false;
        }

        if received_id == self.challenge_id {
            self.status = ChallengeStatus::Passed;
            true
        } else {
            self.status = ChallengeStatus::Failed;
            false
        }
    }

    /// Checks whether the challenge has expired.
    pub fn is_expired(&self, now: Instant) -> bool {
        self.status == ChallengeStatus::Pending
            && now.duration_since(self.created_at) > self.timeout
    }

    /// Marks the challenge as failed due to timeout.
    pub fn mark_expired(&mut self) {
        if self.status == ChallengeStatus::Pending {
            self.status = ChallengeStatus::Failed;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_challenge_success_flow() {
        let now = Instant::now();
        let mut challenge = ProtocolChallenge::new(0xDEAD_BEEF, Duration::from_secs(3));
        assert_eq!(challenge.status(), ChallengeStatus::Pending);

        assert!(challenge.verify_response(0xDEAD_BEEF, now + Duration::from_millis(500)));
        assert_eq!(challenge.status(), ChallengeStatus::Passed);
    }

    #[test]
    fn test_challenge_wrong_token_fails() {
        let now = Instant::now();
        let mut challenge = ProtocolChallenge::new(0x1234_5678, Duration::from_secs(3));
        assert!(!challenge.verify_response(0x9999_9999, now + Duration::from_millis(100)));
        assert_eq!(challenge.status(), ChallengeStatus::Failed);
    }

    #[test]
    fn test_challenge_timeout_fails() {
        let now = Instant::now();
        let mut challenge = ProtocolChallenge::new(0x42, Duration::from_millis(1000));
        assert!(challenge.is_expired(now + Duration::from_millis(1001)));
        assert!(!challenge.verify_response(0x42, now + Duration::from_millis(1001)));
        assert_eq!(challenge.status(), ChallengeStatus::Failed);
    }
}
