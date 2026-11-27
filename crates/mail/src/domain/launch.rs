//! Read-only launch observation. A database row is never launch authority.
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct LaunchBinding {
    pub(crate) postmaster_epoch: i64,
    pub(crate) database_oid: i64,
    pub(crate) profile_digest: [u8; 32],
    pub(crate) month: String,
    pub(crate) evidence: [u8; 32],
    pub(crate) reconciliation_revision: i64,
    pub(crate) limits: [i64; 4],
    pub(crate) pricing: [i64; 3],
    pub(crate) reconciled_attempts: i64,
}

/// Constructed by the PostgreSQL repository, not from an HTTP form.
/// No mailbox, secret, session or raw provider reply.
#[derive(Clone, Eq, PartialEq)]
pub struct LaunchSnapshot {
    pub(crate) binding: LaunchBinding,
    pub(crate) policy_revision: i64,
    pub(crate) enabled: bool,
    pub(crate) boundary_pause: bool,
    pub(crate) reserved_attempts: i64,
    pub(crate) room_for_reservation: bool,
    pub(crate) within_budget: bool,
}

impl LaunchSnapshot {
    pub fn policy_revision(&self) -> i64 {
        self.policy_revision
    }

    pub fn month(&self) -> &str {
        &self.binding.month
    }

    pub fn evidence(&self) -> &[u8; 32] {
        &self.binding.evidence
    }

    pub fn profile_digest(&self) -> &[u8; 32] {
        &self.binding.profile_digest
    }

    pub(crate) fn ready_for_review(&self) -> bool {
        !self.enabled
            && !self.boundary_pause
            && self.room_for_reservation
            && self.binding.postmaster_epoch > 0
            && self.binding.database_oid > 0
            && self.binding.evidence != [0; 32]
            && self.binding.profile_digest != [0; 32]
            && self.binding.reconciliation_revision > 0
            && self.binding.reconciliation_revision == self.policy_revision
            && self.reserved_attempts == self.binding.reconciled_attempts
            && self.reserved_attempts >= 0
            && self.policy_revision.checked_add(1).is_some()
    }
}
