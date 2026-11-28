//! One-shot test admission. A charged reservation is not a send-start witness.
use super::{LaunchSnapshot, REVIEW_LIMIT};
use std::time::Instant;

/// Non-serializable process grant for one administrator's explicit test.
/// A database snapshot or arbitrary monthly count cannot recreate it.
pub struct OperatorTestGrant {
    pub(crate) actor: i64,
    pub(crate) session: [u8; 32],
    pub(crate) before: LaunchSnapshot,
    pub(crate) started: Instant,
}

impl OperatorTestGrant {
    pub fn actor(&self) -> i64 {
        self.actor
    }

    pub(crate) fn matches_reserved(&self, current: &LaunchSnapshot) -> bool {
        self.started.elapsed() < REVIEW_LIMIT
            && current.enabled
            && !current.boundary_pause
            && current.within_budget
            && current.binding == self.before.binding
            && current.policy_revision == self.before.policy_revision
            && current.activation_digest.is_some()
            && current.activation_digest == self.before.activation_digest
            && self.before.reserved_attempts.checked_add(1) == Some(current.reserved_attempts)
    }

    pub(crate) fn matches(&self, current: &LaunchSnapshot) -> bool {
        self.actor > 0
            && self.session != [0; 32]
            && self.started.elapsed() < REVIEW_LIMIT
            && current == &self.before
            && current.enabled
            && !current.boundary_pause
            && current.room_for_reservation
            && current.within_budget
            && current.activation_digest.is_some()
    }
}
