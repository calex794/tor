//! Read-only launch observation and process-local activation intent.
use super::MailClaim;
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

pub(crate) const REVIEW_LIMIT: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchScope {
    OperatorTest(i64),
    Dispatch,
}
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
    pub(crate) activation_digest: Option<[u8; 32]>,
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
            && self.activation_digest.is_none()
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

/// A nonserializable authorization for one exact leased reference and
/// one expected budget increment. Only LaunchFence can mint it.
pub struct DispatchGrant {
    pub(crate) claim: MailClaim,
    pub(crate) before: LaunchSnapshot,
}

impl DispatchGrant {
    pub fn claim(&self) -> MailClaim {
        self.claim
    }

    pub(crate) fn matches(&self, current: &LaunchSnapshot, reserved: bool) -> bool {
        if !current.enabled
            || current.boundary_pause
            || !current.within_budget
            || current.binding != self.before.binding
            || current.policy_revision != self.before.policy_revision
            || current.activation_digest.is_none()
            || current.activation_digest != self.before.activation_digest
        {
            return false;
        }
        if reserved {
            self.before.reserved_attempts.checked_add(1) == Some(current.reserved_attempts)
        } else {
            current == &self.before && current.room_for_reservation
        }
    }

    pub(crate) fn digest(&self) -> Option<&[u8; 32]> {
        self.before.activation_digest.as_ref()
    }
}

/// Produced only after an exact preparation transaction has committed.
/// The secret itself never crosses this boundary.
pub struct PreparedDispatch {
    pub(crate) grant: DispatchGrant,
    pub(crate) marker: String,
    pub(crate) reset_hash: Option<[u8; 32]>,
    pub(crate) started: Instant,
}

impl PreparedDispatch {
    pub(crate) fn new(
        grant: &DispatchGrant,
        marker: String,
        reset_hash: Option<[u8; 32]>,
        started: Instant,
    ) -> Self {
        Self {
            grant: DispatchGrant {
                claim: grant.claim,
                before: grant.before.clone(),
            },
            marker,
            reset_hash,
            started,
        }
    }

    pub fn claim(&self) -> MailClaim {
        self.grant.claim
    }

    pub fn marker(&self) -> &str {
        &self.marker
    }

    pub(crate) fn fresh(&self) -> bool {
        self.started.elapsed() < Duration::from_secs(4)
    }
}

/// A fresh exact database send-start observation. No public constructor,
/// serialization, clone or recovery from a stored counter.
pub struct DispatchStart {
    pub(crate) prepared: PreparedDispatch,
    pub(crate) current: LaunchSnapshot,
}

/// One process-bound, expiring intent. Never deserialize from browser input
/// or persist it in the recovered database. It is not a send authorization.
pub struct ActivationIntent {
    pub(crate) reviewed: LaunchSnapshot,
    pub(crate) scope: LaunchScope,
    pub(crate) session: [u8; 32],
    pub(crate) challenge: [u8; 32],
    pub(crate) started: Instant,
}

impl ActivationIntent {
    pub fn policy_revision(&self) -> i64 {
        self.reviewed.policy_revision
    }

    pub(crate) fn fresh(&self) -> bool {
        self.started.elapsed() < REVIEW_LIMIT
    }

    pub fn proof_digest(&self) -> [u8; 32] {
        let b = &self.reviewed.binding;
        let mut h = Sha256::new();
        h.update(b"tor:mail:launch-intent:v1");
        h.update(self.challenge);
        h.update(self.session);
        let (mode, recipient) = match self.scope {
            LaunchScope::Dispatch => (0u8, 0i64),
            LaunchScope::OperatorTest(id) => (1u8, id),
        };
        h.update([mode]);
        h.update(recipient.to_be_bytes());
        h.update(self.reviewed.policy_revision.to_be_bytes());
        h.update(b.postmaster_epoch.to_be_bytes());
        h.update(b.database_oid.to_be_bytes());
        h.update(b.profile_digest);
        h.update((b.month.len() as u64).to_be_bytes());
        h.update(b.month.as_bytes());
        h.update(b.evidence);
        h.update(b.reconciliation_revision.to_be_bytes());
        for v in b.limits.into_iter().chain(b.pricing) {
            h.update(v.to_be_bytes());
        }
        h.update(b.reconciled_attempts.to_be_bytes());
        h.update(self.reviewed.reserved_attempts.to_be_bytes());
        h.finalize().into()
    }
}
