//! DROP-05b: durable activation claims for already verified R4a PACKs.
//! Separate from R4a build leases: a succeeded build cannot be reclaimed.
//! There is no active-pointer mutation or startup wiring in this slice.
use super::{free_pack_attach::PgFreePackCatalog, free_pack_jobs::PackJobClaim};
use crate::services::authoring::AuthorError;
use sqlx::Row;
use tor_contracts::drop_pack::PackPlacementReceipt;

#[derive(Clone, Debug)]
pub struct PackActivationClaim {
    pub pack: PackJobClaim,
    pub activation_epoch: i64,
    pub attempts: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivationDisposition {
    Retryable,
    Terminal,
}

fn map_error(error: sqlx::Error) -> AuthorError {
    if let sqlx::Error::Database(ref db) = error {
        if matches!(db.code().as_deref(), Some("23503" | "23505" | "23514")) {
            return AuthorError::Conflict;
        }
    }
    AuthorError::Unavailable
}

impl PgFreePackCatalog {
    /// Reserve time to read the full immutable candidate and sidecar. A
    /// queued worker must still own its activation epoch at admission.
    pub async fn reserve_pack_activation_reproof(
        &self,
        claim: &PackJobClaim,
        activation_epoch: Option<i64>,
    ) -> Result<(), AuthorError> {
        if claim.job_id <= 0 || claim.epoch <= 0 || activation_epoch.is_some_and(|epoch| epoch <= 0)
        {
            return Err(AuthorError::Invalid);
        }
        let admitted: bool =
            sqlx::query_scalar("SELECT public.reserve_free_pack_activation_reproof($1,$2,$3,$4)")
                .bind(claim.job_id)
                .bind(claim.epoch)
                .bind(Self::fingerprint(&claim.receipt).as_slice())
                .bind(activation_epoch)
                .fetch_one(&self.pool)
                .await
                .map_err(map_error)?;
        if !admitted {
            return Err(AuthorError::Conflict);
        }
        Ok(())
    }

    /// Fails closed when the additive schema is absent. Does not probe the
    /// remote provider or enable any background task.
    pub async fn probe_pack_activation_schema(&self) -> Result<(), AuthorError> {
        let installed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM information_schema.columns
             WHERE table_schema=current_schema()
               AND table_name='free_pack_activation_jobs'
               AND column_name='lease_epoch')
             AND to_regprocedure(
                 'public.reserve_free_pack_activation_reproof(bigint,bigint,bytea,bigint)'
             ) IS NOT NULL",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(map_error)?;
        if installed {
            Ok(())
        } else {
            Err(AuthorError::Unavailable)
        }
    }

    /// Lock the parent succeeded R4a job before its activation row, so other
    /// B operations can acquire locks in the same order. A crash leaves an
    /// expired lease reclaimable at a strictly greater activation epoch.
    pub async fn claim_pack_activation(
        &self,
        lease_seconds: i32,
    ) -> Result<Option<PackActivationClaim>, AuthorError> {
        if !(15..=900).contains(&lease_seconds) {
            return Err(AuthorError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(map_error)?;
        // Always acquire the parent R4a job lock before its activation row.
        // A bulk UPDATE of activation rows first could deadlock against B's
        // parent-job -> activation-row -> edition locks during a cutover.
        sqlx::query(
            "WITH expired AS (
                SELECT j.id FROM free_pack_jobs j
                JOIN free_pack_activation_jobs a ON a.pack_job_id=j.id
                WHERE j.status='succeeded' AND a.status='running'
                  AND a.attempts>=8 AND a.locked_until<=clock_timestamp()
                ORDER BY j.id FOR UPDATE OF j SKIP LOCKED LIMIT 32
             )
             UPDATE free_pack_activation_jobs a
             SET status='failed',locked_until=NULL,completed_at=clock_timestamp(),
                 updated_at=clock_timestamp(),failure_code='retry_budget_exhausted'
             FROM expired e WHERE a.pack_job_id=e.id
               AND a.status='running' AND a.attempts>=8
               AND a.locked_until<=clock_timestamp()",
        )
        .execute(&mut *tx)
        .await
        .map_err(map_error)?;

        let id: Option<i64> = sqlx::query_scalar(
            "SELECT j.id FROM free_pack_jobs j
             JOIN free_pack_activation_jobs a ON a.pack_job_id=j.id
             WHERE j.status='succeeded' AND j.pack_id IS NOT NULL
               AND a.attempts<8 AND a.retry_after<=clock_timestamp()
               AND (a.status='pending'
                    OR (a.status='running' AND a.locked_until<=clock_timestamp()))
             ORDER BY a.retry_after,a.pack_job_id
             FOR UPDATE OF j SKIP LOCKED LIMIT 1",
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_error)?;
        let Some(id) = id else {
            tx.commit().await.map_err(map_error)?;
            return Ok(None);
        };
        let lease = sqlx::query(
            "UPDATE free_pack_activation_jobs
             SET status='running',lease_epoch=lease_epoch+1,attempts=attempts+1,
                 locked_until=clock_timestamp()+($2::int*interval '1 second'),
                 updated_at=clock_timestamp(),completed_at=NULL,failure_code=NULL
             WHERE pack_job_id=$1 AND attempts<8 AND retry_after<=clock_timestamp()
               AND (status='pending'
                    OR (status='running' AND locked_until<=clock_timestamp()))
             RETURNING lease_epoch,attempts",
        )
        .bind(id)
        .bind(lease_seconds)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_error)?
        .ok_or(AuthorError::Conflict)?;
        let activation_epoch: i64 = lease.try_get("lease_epoch").map_err(map_error)?;
        let attempts: i32 = lease.try_get("attempts").map_err(map_error)?;

        let job = sqlx::query(
            "SELECT j.actor_user_id,j.book_id,j.lease_epoch,j.approved_fingerprint,
                    j.expected_chapter_count,s.spec_text
             FROM free_pack_jobs j JOIN free_pack_job_specs s ON s.job_id=j.id
             WHERE j.id=$1 AND j.status='succeeded' AND j.pack_id IS NOT NULL",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_error)?
        .ok_or(AuthorError::Conflict)?;
        let actor: i64 = job.try_get("actor_user_id").map_err(map_error)?;
        let book: i64 = job.try_get("book_id").map_err(map_error)?;
        let pack_epoch: i64 = job.try_get("lease_epoch").map_err(map_error)?;
        let fingerprint: Vec<u8> = job.try_get("approved_fingerprint").map_err(map_error)?;
        let count: i32 = job.try_get("expected_chapter_count").map_err(map_error)?;
        let spec: String = job.try_get("spec_text").map_err(map_error)?;
        let receipt: PackPlacementReceipt =
            serde_json::from_str(&spec).map_err(|_| AuthorError::Conflict)?;
        if actor <= 0
            || book <= 0
            || pack_epoch <= 0
            || count as usize != receipt.frames.len()
            || Self::fingerprint(&receipt).as_slice() != fingerprint.as_slice()
        {
            return Err(AuthorError::Conflict);
        }
        tx.commit().await.map_err(map_error)?;
        Ok(Some(PackActivationClaim {
            pack: PackJobClaim {
                job_id: id,
                epoch: pack_epoch,
                actor_user_id: actor,
                book_id: book,
                receipt,
            },
            activation_epoch,
            attempts,
        }))
    }

    pub async fn renew_pack_activation(
        &self,
        claim: &PackActivationClaim,
        lease_seconds: i32,
    ) -> Result<bool, AuthorError> {
        if !(15..=900).contains(&lease_seconds)
            || claim.pack.job_id <= 0
            || claim.activation_epoch <= 0
        {
            return Err(AuthorError::Invalid);
        }
        let affected = sqlx::query(
            "UPDATE free_pack_activation_jobs
             SET locked_until=clock_timestamp()+($3::int*interval '1 second'),
                 updated_at=clock_timestamp()
             WHERE pack_job_id=$1 AND lease_epoch=$2 AND status='running'
               AND locked_until>clock_timestamp()",
        )
        .bind(claim.pack.job_id)
        .bind(claim.activation_epoch)
        .bind(lease_seconds)
        .execute(&self.pool)
        .await
        .map_err(map_error)?;
        Ok(affected.rows_affected() == 1)
    }

    /// Only the current activation epoch may settle a failed attempt. No
    /// ordinary code can report Completed: ONLY B's same-transaction
    /// all-member pointer CAS may mark an activation succeeded.
    pub async fn settle_pack_activation(
        &self,
        claim: &PackActivationClaim,
        disposition: ActivationDisposition,
    ) -> Result<bool, AuthorError> {
        if claim.pack.job_id <= 0 || claim.activation_epoch <= 0 {
            return Err(AuthorError::Invalid);
        }
        let affected = match disposition {
            ActivationDisposition::Terminal => sqlx::query(
                "UPDATE free_pack_activation_jobs
                 SET status='failed',locked_until=NULL,completed_at=clock_timestamp(),
                     updated_at=clock_timestamp(),failure_code='terminal_verification'
                 WHERE pack_job_id=$1 AND lease_epoch=$2 AND status='running'
                   AND locked_until>clock_timestamp()",
            )
            .bind(claim.pack.job_id)
            .bind(claim.activation_epoch)
            .execute(&self.pool)
            .await
            .map_err(map_error)?,
            ActivationDisposition::Retryable => sqlx::query(
                "UPDATE free_pack_activation_jobs
                 SET status=CASE WHEN attempts>=8 THEN 'failed' ELSE 'pending' END,
                     locked_until=NULL,
                     completed_at=CASE WHEN attempts>=8
                         THEN clock_timestamp() ELSE NULL END,
                     retry_after=clock_timestamp() +
                         (LEAST(900,15*power(2,LEAST(attempts-1,6)))::int*interval '1 second'),
                     updated_at=clock_timestamp(),
                     failure_code=CASE WHEN attempts>=8
                         THEN 'retry_budget_exhausted' ELSE NULL END
                 WHERE pack_job_id=$1 AND lease_epoch=$2 AND status='running'
                   AND locked_until>clock_timestamp()",
            )
            .bind(claim.pack.job_id)
            .bind(claim.activation_epoch)
            .execute(&self.pool)
            .await
            .map_err(map_error)?,
        };
        Ok(affected.rows_affected() == 1)
    }
}
