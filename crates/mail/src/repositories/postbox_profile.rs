//! Stopped-only candidate custody. Does not verify or activate Postbox.
use crate::domain::postbox_profile::{ProfileIdentity, SealedProfile};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use tor_contracts::auth::POSTBOX_PROFILE_STEP_UP_SCOPE;

type CandidateRow = (String, String, i32, Vec<u8>, Vec<u8>);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileError {
    Invalid,
    Forbidden,
    Conflict,
    StepUpRequired,
    Unavailable,
}

impl From<sqlx::Error> for ProfileError {
    fn from(_: sqlx::Error) -> Self {
        Self::Unavailable
    }
}

pub struct ProfileView {
    pub revision: i64,
    pub identity: Option<ProfileIdentity>,
    pub key_version: Option<i32>,
}

#[derive(Clone)]
pub struct PgPostboxProfile {
    pool: PgPool,
}

impl PgPostboxProfile {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// No access key, secret, nonce or ciphertext crosses the view boundary.
    pub async fn admin_view(&self, digest: &[u8; 32]) -> Result<ProfileView, ProfileError> {
        let mut tx = self.pool.begin().await?;
        let _actor = operator(&mut tx, digest).await?;
        let row: (i64, Option<String>, Option<String>, Option<i32>) = sqlx::query_as(
            "SELECT revision,identity_id,sender,key_version FROM mail_postbox_profile WHERE singleton",
        )
        .fetch_one(&mut *tx)
        .await?;
        let identity = match (row.1, row.2, row.3) {
            (Some(id), Some(sender), Some(version)) if version > 0 => {
                Some(ProfileIdentity::new(id, sender).map_err(|_| ProfileError::Unavailable)?)
            }
            (None, None, None) => None,
            _ => return Err(ProfileError::Unavailable),
        };
        tx.commit().await?;
        Ok(ProfileView {
            revision: row.0,
            identity,
            key_version: row.3,
        })
    }

    /// Save a new *unverified* candidate only after fresh exact-intent
    /// step-up. A consumed proof, stale revision, or enabled policy fails.
    pub async fn admin_save_candidate(
        &self,
        digest: &[u8; 32],
        proof: &str,
        expected_revision: i64,
        candidate: &SealedProfile,
    ) -> Result<(), ProfileError> {
        if expected_revision.checked_add(1) != Some(candidate.revision()) || !candidate.valid() {
            return Err(ProfileError::Invalid);
        }
        if proof.len() != 43
            || !proof
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            return Err(ProfileError::StepUpRequired);
        }
        let proof_hash = Sha256::digest(proof.as_bytes());
        let intent = candidate.intent_digest();
        let mut tx = self.pool.begin().await?;
        let actor = operator(&mut tx, digest).await?;
        stopped(&mut tx).await?;

        let revision: i64 = sqlx::query_scalar(
            "SELECT revision FROM mail_postbox_profile WHERE singleton FOR UPDATE",
        )
        .fetch_one(&mut *tx)
        .await?;
        if revision != expected_revision {
            tx.rollback().await?;
            return Err(ProfileError::Conflict);
        }
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
        .bind(POSTBOX_PROFILE_STEP_UP_SCOPE)
        .bind(expected_revision)
        .bind(intent.as_slice())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if consumed != 1 {
            tx.rollback().await?;
            return Err(ProfileError::StepUpRequired);
        }
        let changed = sqlx::query(
            "UPDATE mail_postbox_profile SET revision=$1,identity_id=$2,sender=$3,
             key_version=$4,nonce=$5,ciphertext=$6,saved_at=clock_timestamp()
             WHERE singleton AND revision=$7",
        )
        .bind(candidate.revision())
        .bind(candidate.identity().identity_id())
        .bind(candidate.identity().sender())
        .bind(candidate.key_version())
        .bind(candidate.nonce.as_slice())
        .bind(&candidate.ciphertext)
        .bind(expected_revision)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed != 1 {
            tx.rollback().await?;
            return Err(ProfileError::Conflict);
        }
        sqlx::query(
            "INSERT INTO mail_postbox_profile_audit
             (actor_user_id,old_revision,new_revision,key_version) VALUES($1,$2,$3,$4)",
        )
        .bind(actor)
        .bind(expected_revision)
        .bind(candidate.revision())
        .bind(candidate.key_version())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Internal administrator probe adapter, not an HTTP response model.
    /// No network I/O while the DB lock is held.
    pub async fn admin_load_candidate(
        &self,
        digest: &[u8; 32],
        expected_revision: i64,
    ) -> Result<SealedProfile, ProfileError> {
        let mut tx = self.pool.begin().await?;
        let _actor = operator(&mut tx, digest).await?;
        stopped(&mut tx).await?;
        let row: Option<CandidateRow> = sqlx::query_as(
            "SELECT identity_id,sender,key_version,nonce,ciphertext FROM mail_postbox_profile
             WHERE singleton AND revision=$1 AND ciphertext IS NOT NULL",
        )
        .bind(expected_revision)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((identity, sender, key_version, nonce, ciphertext)) = row else {
            tx.rollback().await?;
            return Err(ProfileError::Conflict);
        };
        let result = SealedProfile {
            identity: ProfileIdentity::new(identity, sender)
                .map_err(|_| ProfileError::Unavailable)?,
            revision: expected_revision,
            key_version,
            nonce: nonce.try_into().map_err(|_| ProfileError::Unavailable)?,
            ciphertext,
        };
        if !result.valid() {
            return Err(ProfileError::Unavailable);
        }
        tx.commit().await?;
        Ok(result)
    }

    /// Recheck authority, stop policy and the exact sealed candidate after
    /// network I/O. This is informational, not a persisted activation proof.
    pub async fn admin_confirm_probe(
        &self,
        digest: &[u8; 32],
        candidate: &SealedProfile,
    ) -> Result<(), ProfileError> {
        let mut tx = self.pool.begin().await?;
        let _actor = operator(&mut tx, digest).await?;
        stopped(&mut tx).await?;
        let exact: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM mail_postbox_profile WHERE singleton
             AND revision=$1 AND identity_id=$2 AND sender=$3
             AND key_version=$4 AND nonce=$5 AND ciphertext=$6)",
        )
        .bind(candidate.revision())
        .bind(candidate.identity().identity_id())
        .bind(candidate.identity().sender())
        .bind(candidate.key_version())
        .bind(candidate.nonce.as_slice())
        .bind(&candidate.ciphertext)
        .fetch_one(&mut *tx)
        .await?;
        if !exact {
            tx.rollback().await?;
            return Err(ProfileError::Conflict);
        }
        tx.commit().await?;
        Ok(())
    }
}

pub(super) async fn operator(
    tx: &mut Transaction<'_, Postgres>,
    digest: &[u8; 32],
) -> Result<i64, ProfileError> {
    // Match accounts' lock order: role/account first, then exclusive session.
    let actor: Option<i64> = sqlx::query_scalar(
        "SELECT u.id FROM users u JOIN sessions s ON s.user_id=u.id
         JOIN account_roles r ON r.user_id=u.id AND r.role='administrator' AND r.is_active
         WHERE s.token_hash=$1 AND s.revoked_at IS NULL
           AND s.expires_at>clock_timestamp() AND u.account_state='active'
         FOR SHARE OF u,r",
    )
    .bind(digest.as_slice())
    .fetch_optional(&mut **tx)
    .await?;
    let actor = actor.ok_or(ProfileError::Forbidden)?;
    sqlx::query_scalar(
        "SELECT user_id FROM sessions WHERE token_hash=$1 AND user_id=$2
         AND revoked_at IS NULL AND expires_at>clock_timestamp() FOR UPDATE",
    )
    .bind(digest.as_slice())
    .bind(actor)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ProfileError::Forbidden)
}

async fn stopped(tx: &mut Transaction<'_, Postgres>) -> Result<(), ProfileError> {
    sqlx::query_scalar::<_, bool>(
        "SELECT singleton FROM mail_send_budget_guard WHERE singleton FOR UPDATE",
    )
    .fetch_one(&mut **tx)
    .await?;
    let enabled: bool = sqlx::query_scalar("SELECT enabled FROM mail_cost_policy WHERE singleton")
        .fetch_one(&mut **tx)
        .await?;
    if enabled {
        return Err(ProfileError::Conflict);
    }
    Ok(())
}
