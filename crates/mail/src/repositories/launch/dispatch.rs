//! Caller holds the common quota guard throughout validation and mutation.
use super::load;
use crate::domain::launch::LaunchSnapshot;
use sqlx::{Postgres, Transaction};

pub(in crate::repositories) async fn has_activation_receipt(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM mail_cost_policy p
         JOIN mail_launch_activation_audit a ON a.new_revision=p.revision
         WHERE p.singleton AND p.enabled)",
    )
    .fetch_one(&mut **tx)
    .await
}

/// Validate the exact observed budget/profile/epoch and the durable *dispatch*
/// mode. Operator-test receipts cannot authorize this path. This is a second
/// DB fence; the worker must separately hold process-local authority.
pub(in crate::repositories) async fn dispatch_matches(
    tx: &mut Transaction<'_, Postgres>,
    expected: &LaunchSnapshot,
    for_reservation: bool,
) -> Result<bool, sqlx::Error> {
    let Some(digest) = expected.activation_digest else {
        return Ok(false);
    };
    if !expected.enabled
        || expected.boundary_pause
        || !expected.within_budget
        || (for_reservation && !expected.room_for_reservation)
    {
        return Ok(false);
    }
    let Some(current) = load(tx).await? else {
        return Ok(false);
    };
    if &current != expected {
        return Ok(false);
    }
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM mail_launch_activation_audit
         WHERE new_revision=$1 AND intent_digest=$2
           AND mode='dispatch' AND test_user_id IS NULL)",
    )
    .bind(expected.policy_revision())
    .bind(digest.as_slice())
    .fetch_one(&mut **tx)
    .await
}
