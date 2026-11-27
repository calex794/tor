//! A process-local, fail-closed second gate. It cannot enable PostgreSQL.
//! All web/worker clones must share a single instance. Never deserialize it.
use crate::domain::launch::LaunchSnapshot;
use rand::{rngs::OsRng, RngCore};
use std::{
    mem,
    sync::Mutex,
    time::{Duration, Instant},
};

const REVIEW_LIMIT: Duration = Duration::from_secs(300);

/// A test authorization is tied to one trusted administrator user ID and
/// cannot be used for registration, support or news delivery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchScope {
    OperatorTest(i64),
    Dispatch,
}

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

    /// Caller rechecks live administrator authority, fresh password proof,
    /// successful non-sending probe of this exact profile, and explicitly
    /// reviewed independent billing/restore evidence. The supplied hash is a
    /// reference to that review, NOT a proof of its authenticity.
    ///
    /// The future activation transaction must advance the reviewed policy
    /// revision by exactly one, leave all other bindings unchanged and only
    /// then set enabled=true. This function performs no database mutation.
    pub fn confirm_review(
        &self,
        challenge: &[u8; 32],
        session: &[u8; 32],
        current: &LaunchSnapshot,
        external_evidence: &[u8; 32],
        probed_profile: &[u8; 32],
    ) -> Result<(), LaunchError> {
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
        *state = State::Armed(Approval {
            reviewed: review.snapshot,
            scope: review.scope,
            high_water: current.reserved_attempts,
        });
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
        if !matches_approval(approval, current, scope) || !current.room_for_reservation {
            *state = State::Stopped;
            return Err(LaunchError::Conflict);
        }
        Ok(())
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
        let mut state = self.state.lock().map_err(|_| LaunchError::Unavailable)?;
        let State::Armed(mut approval) = mem::take(&mut *state) else {
            return Err(LaunchError::Stopped);
        };
        if !matches_approval(&approval, current, scope)
            || current.reserved_attempts <= approval.high_water
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

fn matches_approval(
    approval: &Approval,
    current: &LaunchSnapshot,
    scope: LaunchScope,
) -> bool {
    approval.scope == scope
        && current.enabled
        && !current.boundary_pause
        && current.within_budget
        && current.binding == approval.reviewed.binding
        && approval.reviewed.policy_revision.checked_add(1) == Some(current.policy_revision)
        && current.reserved_attempts >= approval.high_water
}

#[cfg(test)]
mod tests;
