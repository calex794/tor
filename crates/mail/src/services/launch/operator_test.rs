//! One irreversible test grant; no network or reservation here.
use super::{matches_approval, LaunchError, LaunchFence, LaunchScope, State, REVIEW_LIMIT};
use crate::domain::launch::{LaunchSnapshot, OperatorTestGrant};
use std::time::Instant;

pub(super) struct TestApproval {
    actor: i64,
    session: [u8; 32],
    activation_digest: [u8; 32],
    started: Instant,
}

impl LaunchFence {
    /// Move the exact test-only approval into a single in-progress attempt.
    /// Repeated requests cannot obtain a second grant or arm normal dispatch.
    pub fn authorize_operator_test(
        &self,
        current: &LaunchSnapshot,
        actor: i64,
        session: &[u8; 32],
    ) -> Result<OperatorTestGrant, LaunchError> {
        let mut state = self.state.lock().map_err(|_| LaunchError::Unavailable)?;
        let State::Armed(approval) = &*state else {
            return Err(LaunchError::Stopped);
        };
        if actor <= 0
            || &approval.session != session
            || approval.started.elapsed() >= REVIEW_LIMIT
            || !matches_approval(approval, current, LaunchScope::OperatorTest(actor))
            || current.reserved_attempts != approval.high_water
            || !current.room_for_reservation
        {
            *state = State::Stopped;
            return Err(LaunchError::Conflict);
        }
        let grant = OperatorTestGrant {
            actor,
            session: *session,
            before: current.clone(),
            started: approval.started,
        };
        *state = State::Testing(TestApproval {
            actor,
            session: *session,
            activation_digest: approval.activation_digest,
            started: approval.started,
        });
        Ok(grant)
    }

    /// Check before any future network operation. The eventual reservation
    /// and send-start transaction must still check live actor and exact receipt.
    pub fn precheck_operator_test(
        &self,
        grant: &OperatorTestGrant,
        current: &LaunchSnapshot,
    ) -> Result<(), LaunchError> {
        let state = self.state.lock().map_err(|_| LaunchError::Unavailable)?;
        let State::Testing(approval) = &*state else {
            return Err(LaunchError::Stopped);
        };
        if !grant.matches(current)
            || grant.actor != approval.actor
            || grant.session != approval.session
            || grant.started != approval.started
            || current.activation_digest != Some(approval.activation_digest)
        {
            return Err(LaunchError::Conflict);
        }
        Ok(())
    }
}
