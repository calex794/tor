//! Caller holds the common quota guard throughout validation and mutation.
use super::{invalid, load, lock_guard, PgMailLaunch};
use crate::domain::{
    launch::LaunchSnapshot,
    postbox_profile::{ProfileIdentity, SealedProfile},
};
use sqlx::{Postgres, Row, Transaction};

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

impl PgMailLaunch {
    /// Internal worker read only. The sealed profile must match the exact
    /// active dispatch snapshot; it is never an administrator response.
    pub async fn dispatch_profile(
        &self,
        expected: &LaunchSnapshot,
    ) -> Result<Option<SealedProfile>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        lock_guard(&mut tx).await?;
        if !dispatch_matches(&mut tx, expected, true).await? {
            tx.rollback().await?;
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT revision,identity_id,sender,key_version,nonce,ciphertext
             FROM mail_postbox_profile WHERE singleton AND ciphertext IS NOT NULL",
        )
        .fetch_one(&mut *tx)
        .await?;
        let candidate = SealedProfile {
            identity: ProfileIdentity::new(row.try_get("identity_id")?, row.try_get("sender")?)
                .map_err(|_| invalid())?,
            revision: row.try_get("revision")?,
            key_version: row.try_get("key_version")?,
            nonce: row
                .try_get::<Vec<u8>, _>("nonce")?
                .try_into()
                .map_err(|_| invalid())?,
            ciphertext: row.try_get("ciphertext")?,
        };
        if !candidate.valid() || &candidate.intent_digest() != expected.profile_digest() {
            tx.rollback().await?;
            return Ok(None);
        }
        tx.commit().await?;
        Ok(Some(candidate))
    }
}
