use super::*;
use crate::domain::launch::LaunchBinding;

const SESSION: [u8; 32] = [9; 32];

fn stopped() -> LaunchSnapshot {
    LaunchSnapshot {
        binding: LaunchBinding {
            postmaster_epoch: 100,
            database_oid: 20,
            profile_digest: [3; 32],
            month: "2026-11-01".into(),
            evidence: [4; 32],
            reconciliation_revision: 7,
            limits: [10000, 30000, 40000, 50000],
            pricing: [0, 0, 8032],
            reconciled_attempts: 42,
        },
        policy_revision: 7,
        activation_digest: None,
        enabled: false,
        boundary_pause: false,
        reserved_attempts: 42,
        room_for_reservation: true,
        within_budget: true,
    }
}

fn armed(scope: LaunchScope) -> (LaunchFence, LaunchSnapshot) {
    let gate = LaunchFence::new();
    let mut snapshot = stopped();
    let challenge = gate.begin_review(&snapshot, scope, SESSION).unwrap();
    let intent = gate
        .confirm_review(
            &challenge,
            &SESSION,
            &snapshot,
            snapshot.evidence(),
            snapshot.profile_digest(),
        )
        .unwrap();
    snapshot.enabled = true;
    snapshot.policy_revision += 1;
    snapshot.activation_digest = Some(intent.proof_digest());
    gate.complete_activation(intent, &snapshot).unwrap();
    (gate, snapshot)
}

#[test]
fn restored_enabled_database_and_new_process_are_stopped() {
    let (gate, mut current) = armed(LaunchScope::Dispatch);
    gate.permit_reservation(&current, LaunchScope::Dispatch)
        .unwrap();
    current.reserved_attempts += 1;
    gate.consume_start(&current, LaunchScope::Dispatch).unwrap();
    let restarted = LaunchFence::new();
    assert_eq!(
        restarted.permit_reservation(&current, LaunchScope::Dispatch),
        Err(LaunchError::Stopped)
    );
    assert_eq!(
        restarted.begin_review(&current, LaunchScope::Dispatch, SESSION),
        Err(LaunchError::NotReady)
    );
}

#[test]
fn external_review_is_exact_session_bound_and_single_use() {
    let gate = LaunchFence::new();
    let snapshot = stopped();
    let challenge = gate
        .begin_review(&snapshot, LaunchScope::Dispatch, SESSION)
        .unwrap();
    assert_eq!(
        gate
            .confirm_review(
                &challenge,
                &[8; 32],
                &snapshot,
                snapshot.evidence(),
                snapshot.profile_digest(),
            )
            .err(),
        Some(LaunchError::Conflict)
    );
    assert_eq!(
        gate
            .confirm_review(
                &challenge,
                &SESSION,
                &snapshot,
                snapshot.evidence(),
                snapshot.profile_digest(),
            )
            .err(),
        Some(LaunchError::Stopped)
    );
    let challenge = gate
        .begin_review(&snapshot, LaunchScope::Dispatch, SESSION)
        .unwrap();
    assert_eq!(
        gate
            .confirm_review(
                &challenge,
                &SESSION,
                &snapshot,
                &[7; 32],
                snapshot.profile_digest(),
            )
            .err(),
        Some(LaunchError::Conflict)
    );
    let challenge = gate
        .begin_review(&snapshot, LaunchScope::Dispatch, SESSION)
        .unwrap();
    let mut state = gate.state.lock().unwrap();
    let State::Reviewing(review) = &mut *state else {
        panic!("missing review");
    };
    review.started -= REVIEW_LIMIT;
    drop(state);
    assert_eq!(
        gate
            .confirm_review(
                &challenge,
                &SESSION,
                &snapshot,
                snapshot.evidence(),
                snapshot.profile_digest(),
            )
            .err(),
        Some(LaunchError::Conflict)
    );
}

#[test]
fn test_authority_is_one_use_and_not_a_dispatch_grant() {
    let scope = LaunchScope::OperatorTest(7);
    let (gate, mut snapshot) = armed(scope);
    assert_eq!(
        gate.permit_reservation(&snapshot, LaunchScope::Dispatch),
        Err(LaunchError::Conflict)
    );
    let (gate, active) = armed(scope);
    snapshot = active;
    gate.permit_reservation(&snapshot, scope).unwrap();
    snapshot.reserved_attempts += 1;
    gate.consume_start(&snapshot, scope).unwrap();
    snapshot.reserved_attempts += 1;
    assert_eq!(
        gate.consume_start(&snapshot, scope),
        Err(LaunchError::Stopped)
    );
}

#[test]
fn restart_rollover_profile_revision_or_counter_rollback_latches_stop() {
    type Alteration = fn(&mut LaunchSnapshot);
    let changes: [Alteration; 10] = [
        |s| s.binding.postmaster_epoch += 1,
        |s| s.binding.database_oid += 1,
        |s| s.binding.profile_digest = [6; 32],
        |s| s.binding.month = "2026-12-01".into(),
        |s| s.binding.evidence = [6; 32],
        |s| s.policy_revision += 1,
        |s| s.reserved_attempts -= 1,
        |s| s.binding.pricing[0] += 1,
        |s| s.boundary_pause = true,
        |s| s.enabled = false,
    ];
    for change in changes {
        let (gate, current) = armed(LaunchScope::Dispatch);
        let mut changed = current.clone();
        change(&mut changed);
        assert_eq!(
            gate.permit_reservation(&changed, LaunchScope::Dispatch),
            Err(LaunchError::Conflict)
        );
        assert_eq!(
            gate.permit_reservation(&current, LaunchScope::Dispatch),
            Err(LaunchError::Stopped)
        );
    }
}

#[test]
fn each_send_start_requires_new_committed_counter_and_explicit_stop_works() {
    let (gate, mut current) = armed(LaunchScope::Dispatch);
    current.reserved_attempts += 1;
    gate.consume_start(&current, LaunchScope::Dispatch).unwrap();
    assert_eq!(
        gate.consume_start(&current, LaunchScope::Dispatch),
        Err(LaunchError::Conflict)
    );
    let (gate, current) = armed(LaunchScope::Dispatch);
    gate.stop().unwrap();
    assert_eq!(
        gate.permit_reservation(&current, LaunchScope::Dispatch),
        Err(LaunchError::Stopped)
    );
}

#[test]
fn no_commit_no_approval_and_stop_wins_over_late_commit() {
    let gate = LaunchFence::new();
    let mut snapshot = stopped();
    let challenge = gate
        .begin_review(&snapshot, LaunchScope::Dispatch, SESSION)
        .unwrap();
    let intent = gate
        .confirm_review(
            &challenge,
            &SESSION,
            &snapshot,
            snapshot.evidence(),
            snapshot.profile_digest(),
        )
        .unwrap();
    snapshot.enabled = true;
    snapshot.policy_revision += 1;
    snapshot.activation_digest = Some(intent.proof_digest());
    assert_eq!(
        gate.permit_reservation(&snapshot, LaunchScope::Dispatch),
        Err(LaunchError::Stopped)
    );
    gate.stop().unwrap();
    assert_eq!(
        gate.complete_activation(intent, &snapshot),
        Err(LaunchError::Stopped)
    );
}

#[test]
fn missing_or_different_receipt_and_skipped_counter_are_not_accepted() {
    let (gate, mut current) = armed(LaunchScope::Dispatch);
    current.activation_digest = None;
    assert_eq!(
        gate.permit_reservation(&current, LaunchScope::Dispatch),
        Err(LaunchError::Conflict)
    );
    let (gate, mut current) = armed(LaunchScope::Dispatch);
    current.reserved_attempts += 2;
    assert_eq!(
        gate.consume_start(&current, LaunchScope::Dispatch),
        Err(LaunchError::Conflict)
    );
}
