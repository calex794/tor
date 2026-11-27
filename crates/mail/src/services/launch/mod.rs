//! A process-local, fail-closed second gate. It cannot enable PostgreSQL.
//! All web/worker clones must share a single instance. Never deserialize it.
pub use crate::domain::launch::LaunchScope;
use crate::domain::{
    launch::{ActivationIntent, DispatchGrant, DispatchStart, LaunchSnapshot, REVIEW_LIMIT},
    MailClaim,
};
use rand::{rngs::OsRng, RngCore};
use std::{mem, sync::Mutex, time::Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchError {
    Stopped,
    NotReady,
    Conflict,
    Unavailable,
}

#[derive(Default)]
enum State {
    #[default]
    Stopped,
    Reviewing(Review),
    Confirming([u8; 32]),
    Armed(Approval),
}

struct Review {
    snapshot: LaunchSnapshot,
    scope: LaunchScope,
    session: [u8; 32],
    challenge: [u8; 32],
    started: Instant,
}

struct Approval {
    reviewed: LaunchSnapshot,
    scope: LaunchScope,
    high_water: i64,
    activation_digest: [u8; 32],
}

/// A DB backup cannot restore this state. Every new process is stopped,
/// even if its database contains an enabled policy and old audit receipts.
#[derive(Default)]
pub struct LaunchFence {
    state: Mutex<State>,
}

impl LaunchFence {
    pub fn new() -> Self {
        Self::default()
    }

    /// Caller first authenticates the live administrator. The returned nonce
    /// is scoped to this process, session, mode and exact stopped snapshot.
    /// Starting a new review always revokes a previous approval.
    pub fn begin_review(
        &self,
        snapshot: &LaunchSnapshot,
        scope: LaunchScope,
        session: [u8; 32],
    ) -> Result<[u8; 32], LaunchError> {
        let mut state = self.state.lock().map_err(|_| LaunchError::Unavailable)?;
        *state = State::Stopped;
        if !snapshot.ready_for_review()
            || matches!(scope, LaunchScope::OperatorTest(id) if id <= 0)
            || session == [0; 32]
        {
            return Err(LaunchError::NotReady);
        }
        let mut challenge = [0; 32];
        OsRng
            .try_fill_bytes(&mut challenge)
            .map_err(|_| LaunchError::Unavailable)?;
        *state = State::Reviewing(Review {
            snapshot: snapshot.clone(),
            scope,
            session,
            challenge,
            started: Instant::now(),
        });
        Ok(challenge)
    }

    /// Caller rechecks live administrator authority, successful non-sending
    /// probe of this exact profile, and independently reviewed billing/restore
    /// evidence. The hash is a reference, NOT proof of authenticity. This
    /// produces an intent for the fresh password proof and DB transaction,
    /// but deliberately does not arm sending.
    pub fn confirm_review(
        &self,
        challenge: &[u8; 32],
        session: &[u8; 32],
        current: &LaunchSnapshot,
        external_evidence: &[u8; 32],
        probed_profile: &[u8; 32],
    ) -> Result<ActivationIntent, LaunchError> {
        let mut state = self.state.lock().map_err(|_| LaunchError::Unavailable)?;
        let State::Reviewing(review) = mem::take(&mut *state) else {
            return Err(LaunchError::Stopped);
        };
        if review.started.elapsed() >= REVIEW_LIMIT
            || &review.challenge != challenge
            || &review.session != session
            || &review.snapshot != current
            || !current.ready_for_review()
            || current.evidence() != external_evidence
            || current.profile_digest() != probed_profile
        {
            return Err(LaunchError::Conflict);
        }
        let intent = ActivationIntent {
            reviewed: review.snapshot,
            scope: review.scope,
            session: review.session,
            challenge: review.challenge,
            started: review.started,
        };
        *state = State::Confirming(intent.proof_digest());
        Ok(intent)
    }

    /// Call only after admin_activate commits and a fresh observation
    /// contains its exact durable receipt. A local stop/new review during
    /// the DB transaction wins: an old result cannot resurrect authority.
    pub fn complete_activation(
        &self,
        intent: ActivationIntent,
        current: &LaunchSnapshot,
    ) -> Result<(), LaunchError> {
        let mut state = self.state.lock().map_err(|_| LaunchError::Unavailable)?;
        let State::Confirming(digest) = &*state else {
            return Err(LaunchError::Stopped);
        };
        if *digest != intent.proof_digest() {
            return Err(LaunchError::Conflict);
        }
        let digest = *digest;
        *state = State::Stopped;
        if !intent.fresh() {
            return Err(LaunchError::Conflict);
        }
        let approval = Approval {
            high_water: intent.reviewed.reserved_attempts,
            reviewed: intent.reviewed,
            scope: intent.scope,
            activation_digest: digest,
        };
        if !matches_approval(&approval, current, approval.scope)
            || current.reserved_attempts != approval.high_water
            || !current.room_for_reservation
        {
            return Err(LaunchError::Conflict);
        }
        *state = State::Armed(approval);
        Ok(())
    }

    /// Local fence before a new database reservation. Does not itself reserve
    /// quota. Missing/stale observations must be handled as stop by the caller.
    pub fn permit_reservation(
        &self,
        current: &LaunchSnapshot,
        scope: LaunchScope,
    ) -> Result<(), LaunchError> {
        let mut state = self.state.lock().map_err(|_| LaunchError::Unavailable)?;
        let State::Armed(approval) = &*state else {
            return Err(LaunchError::Stopped);
        };
        if !matches_approval(approval, current, scope)
            || current.reserved_attempts != approval.high_water
            || !current.room_for_reservation
        {
            *state = State::Stopped;
            return Err(LaunchError::Conflict);
        }
        Ok(())
    }

    /// Bind the current process approval to a particular queue claim. The
    /// database must still compare the grant under its quota singleton.
    pub fn authorize_dispatch(
        &self,
        current: &LaunchSnapshot,
        claim: MailClaim,
    ) -> Result<DispatchGrant, LaunchError> {
        if claim.id <= 0 || claim.generation <= 0 {
            return Err(LaunchError::Conflict);
        }
        self.permit_reservation(current, LaunchScope::Dispatch)?;
        Ok(DispatchGrant {
            claim,
            before: current.clone(),
        })
    }

    /// The common worker consumes the same grant after final preparation
    /// and a fresh observation, immediately before starting its SMTP send.
    pub fn consume_dispatch_start(&self, start: DispatchStart) -> Result<(), LaunchError> {
        if !start.prepared.fresh() || !start.prepared.grant.matches(&start.current, true) {
            self.stop()?;
            return Err(LaunchError::Conflict);
        }
        self.consume_counter(&start.current, LaunchScope::Dispatch)
    }

    /// Called only after an exact claim/attempt reservation commits and the
    /// durable send-start/feedback gate succeeds. Fetch a fresh observation
    /// immediately before submission; never hold SQL locks across SMTP.
    /// A replayed/decreasing counter or different mode invalidates approval.
    /// One dispatch worker must serialize this check and actual send start.
    pub fn consume_start(
        &self,
        current: &LaunchSnapshot,
        scope: LaunchScope,
    ) -> Result<(), LaunchError> {
        // Dispatch must present the exact prepared DB witness; a naked
        // counter or a reservation grant cannot authorize a message.
        if scope == LaunchScope::Dispatch {
            self.stop()?;
            return Err(LaunchError::Conflict);
        }
        self.consume_counter(current, scope)
    }

    fn consume_counter(
        &self,
        current: &LaunchSnapshot,
        scope: LaunchScope,
    ) -> Result<(), LaunchError> {
        let mut state = self.state.lock().map_err(|_| LaunchError::Unavailable)?;
        let State::Armed(mut approval) = mem::take(&mut *state) else {
            return Err(LaunchError::Stopped);
        };
        if !matches_approval(&approval, current, scope)
            || approval.high_water.checked_add(1) != Some(current.reserved_attempts)
        {
            return Err(LaunchError::Conflict);
        }
        if scope == LaunchScope::Dispatch {
            approval.high_water = current.reserved_attempts;
            *state = State::Armed(approval);
        }
        // OperatorTest consumes the entire approval, regardless of SMTP
        // result; it can never authorize a second attempt.
        Ok(())
    }

    /// Call before stopping, restoring/reconnecting, or changing the profile
    /// or budget. A poisoned mutex is fail-closed at every admission method.
    pub fn stop(&self) -> Result<(), LaunchError> {
        let mut state = self.state.lock().map_err(|_| LaunchError::Unavailable)?;
        *state = State::Stopped;
        Ok(())
    }
}

fn matches_approval(approval: &Approval, current: &LaunchSnapshot, scope: LaunchScope) -> bool {
    approval.scope == scope
        && current.enabled
        && !current.boundary_pause
        && current.within_budget
        && current.binding == approval.reviewed.binding
        && current.activation_digest == Some(approval.activation_digest)
        && approval.reviewed.policy_revision.checked_add(1) == Some(current.policy_revision)
        && current.reserved_attempts >= approval.high_water
}

#[cfg(test)]
mod tests;
