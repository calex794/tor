//! One explicit, password-protected transaction. No network or worker start.
use super::{load, load_profile, lock_guard, PgMailLaunch};
use crate::{
    domain::{
        launch::{ActivationIntent, LaunchScope, LaunchSnapshot},
        postbox_profile::SealedProfile,
    },
    repositories::postbox_profile::{operator, ProfileError},
};
use sha2::{Digest, Sha256};
use tor_contracts::auth::MAIL_ACTIVATION_STEP_UP_SCOPE;

impl PgMailLaunch {
    /// An exact stopped review and encrypted candidate under one quota lock.
    /// No password proof, network access, policy change or send authorization.
    /// Repeat after a non-sending probe to recheck live role and all bindings.
    pub async fn admin_review_candidate(
        &self,
        digest: &[u8; 32],
    ) -> Result<(LaunchSnapshot, SealedProfile), ProfileError> {
        let mut tx = self.pool.begin().await?;
        operator(&mut tx, digest).await?;
        lock_guard(&mut tx).await?;
        let snapshot = load(&mut tx).await?.ok_or(ProfileError::Conflict)?;
        if !snapshot.ready_for_review() {
            return Err(ProfileError::Conflict);
        }
        let candidate = load_profile(&mut tx).await?;
        if !candidate.valid() || &candidate.intent_digest() != snapshot.profile_digest() {
            return Err(ProfileError::Conflict);
        }
        tx.commit().await?;
        Ok((snapshot, candidate))
    }

    /// The caller must complete the local fence with a fresh observation
    /// after a successful commit. On any failure stop the local fence.
    pub async fn admin_activate(
        &self,
        digest: &[u8; 32],
        proof: &str,
        intent: &ActivationIntent,
    ) -> Result<(), ProfileError> {
        if &intent.session != digest || !intent.fresh() {
            return Err(ProfileError::Conflict);
        }
        if proof.len() != 43
            || !proof
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            return Err(ProfileError::StepUpRequired);
        }
        let mut tx = self.pool.begin().await?;
        let actor = operator(&mut tx, digest).await?;
        let (mode, test_user) = match intent.scope {
            LaunchScope::Dispatch => ("dispatch", None),
            LaunchScope::OperatorTest(id) if id == actor => ("operator_test", Some(id)),
            LaunchScope::OperatorTest(_) => return Err(ProfileError::Forbidden),
        };
        lock_guard(&mut tx).await?;
        let current = load(&mut tx).await?.ok_or(ProfileError::Conflict)?;
        if current != intent.reviewed || !current.ready_for_review() || !intent.fresh() {
            return Err(ProfileError::Conflict);
        }
        let revision = current.policy_revision();
        let next = revision.checked_add(1).ok_or(ProfileError::Unavailable)?;
        let proof_hash = Sha256::digest(proof.as_bytes());
        let intent_digest = intent.proof_digest();
        let consumed = sqlx::query(
            "UPDATE sessions SET step_up_used_at=clock_timestamp()
             WHERE user_id=$1 AND token_hash=$2 AND revoked_at IS NULL
               AND expires_at>clock_timestamp() AND step_up_proof_hash=$3
               AND step_up_scope=$4 AND step_up_resource_id=1
               AND step_up_revision=$5 AND step_up_payload_digest=$6
               AND step_up_issued_at<=clock_timestamp()
               AND step_up_expires_at>clock_timestamp() AND step_up_used_at IS NULL",
        )
        .bind(actor)
        .bind(digest.as_slice())
        .bind(proof_hash.as_slice())
        .bind(MAIL_ACTIVATION_STEP_UP_SCOPE)
        .bind(revision)
        .bind(intent_digest.as_slice())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if consumed != 1 {
            return Err(ProfileError::StepUpRequired);
        }
        let changed = sqlx::query(
            "UPDATE mail_cost_policy SET enabled=true,revision=$1
             WHERE singleton AND NOT enabled AND revision=$2
               AND $3::date=date_trunc('month',clock_timestamp() AT TIME ZONE 'Europe/Moscow')::date
               AND clock_timestamp() AT TIME ZONE 'Europe/Moscow'
                    < $3::date+interval '1 month'-interval '5 minutes'",
        )
        .bind(next)
        .bind(revision)
        .bind(current.month())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed != 1 || !intent.fresh() {
            return Err(ProfileError::Conflict);
        }
        sqlx::query(
            "INSERT INTO mail_launch_activation_audit
             (actor_user_id,old_revision,new_revision,mode,test_user_id,month_start,
              profile_digest,evidence_sha256,intent_digest,postmaster_epoch,database_oid,
              reserved_attempts)
             VALUES($1,$2,$3,$4,$5,$6::date,$7,$8,$9,$10,$11,$12)",
        )
        .bind(actor)
        .bind(revision)
        .bind(next)
        .bind(mode)
        .bind(test_user)
        .bind(current.month())
        .bind(current.profile_digest().as_slice())
        .bind(current.evidence().as_slice())
        .bind(intent_digest.as_slice())
        .bind(current.binding.postmaster_epoch)
        .bind(current.binding.database_oid)
        .bind(current.reserved_attempts)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}
