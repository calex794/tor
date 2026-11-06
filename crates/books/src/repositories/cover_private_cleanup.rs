//! B-owned, durable private-variant cleanup. This is NOT public CDN purge.
 //! No review history, original uploads, or currently attached slots are deleted.
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgPrivateCoverVariantRemovals {
    pool: PgPool,
}

pub struct PrivateVariantRemoval {
    pub id: i64,
    pub job_id: i64,
    pub variant_name: String,
    pub storage_key: String,
    pub lease_epoch: i64,
}

impl PgPrivateCoverVariantRemovals {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Stage only terminal rejected/superseded variants after seven full days.
    /// Never stage the current B slot, even if the source rows are inconsistent.
    /// The anti-join prevents an old page of rows starving newer candidates.
    pub async fn stage_eligible(&self, limit: i64) -> Result<u64, sqlx::Error> {
        if !(1..=20).contains(&limit) {
            return Err(sqlx::Error::Protocol("invalid private-cover cleanup batch".into()));
        }
        let result = sqlx::query(
            "INSERT INTO cover_private_variant_removals(job_id,variant_name,storage_key)
             SELECT v.job_id,v.variant_name,v.storage_key
             FROM book_cover_review_requests r
             JOIN cover_variant_jobs j ON j.id=r.job_id
             JOIN cover_variants v ON v.job_id=j.id
             LEFT JOIN book_cover_slots s ON s.job_id=j.id
             LEFT JOIN cover_private_variant_removals prior
                 ON prior.job_id=v.job_id AND prior.variant_name=v.variant_name
             WHERE prior.id IS NULL AND s.book_id IS NULL
               AND j.status IN ('done','superseded','dead')
               AND (
                 (r.state='rejected' AND r.decided_at<=now()-interval '7 days')
                 OR (r.state='superseded' AND r.submitted_at<=now()-interval '7 days')
               )
             ORDER BY r.id,v.variant_name
             LIMIT $1
             ON CONFLICT DO NOTHING",
        )
        .bind(limit)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// A short fenced lease allows retries after crashes or S3 timeouts.
    /// Re-check terminal decisions and the current attached slot on each claim.
    pub async fn claim(&self, limit: i64) -> Result<Vec<PrivateVariantRemoval>, sqlx::Error> {
        if !(1..=3).contains(&limit) {
            return Err(sqlx::Error::Protocol("invalid private-cover removal lease count".into()));
        }
        let rows = sqlx::query(
            "WITH eligible AS (
                SELECT c.id FROM cover_private_variant_removals c
                JOIN cover_variants v
                  ON v.job_id=c.job_id AND v.variant_name=c.variant_name
                    AND v.storage_key=c.storage_key
                JOIN cover_variant_jobs j ON j.id=c.job_id
                JOIN book_cover_review_requests r ON r.job_id=j.id
                LEFT JOIN book_cover_slots s ON s.job_id=j.id
                WHERE s.book_id IS NULL
                  AND j.status IN ('done','superseded','dead')
                  AND (
                    (r.state='rejected' AND r.decided_at<=now()-interval '7 days')
                    OR (r.state='superseded' AND r.submitted_at<=now()-interval '7 days')
                  )
                  AND (
                    (c.state='pending' AND c.retry_after<=now())
                    OR (c.state='leased' AND c.locked_until<now())
                  )
                ORDER BY c.id
                FOR UPDATE OF c SKIP LOCKED LIMIT $1
             )
             UPDATE cover_private_variant_removals c
             SET state='leased',lease_epoch=c.lease_epoch+1,
                 locked_until=now()+interval '60 seconds'
             FROM eligible e WHERE e.id=c.id
             RETURNING c.id,c.job_id,c.variant_name,c.storage_key,c.lease_epoch",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(PrivateVariantRemoval {
                    id: row.try_get("id")?,
                    job_id: row.try_get("job_id")?,
                    variant_name: row.try_get("variant_name")?,
                    storage_key: row.try_get("storage_key")?,
                    lease_epoch: row.try_get("lease_epoch")?,
                })
            })
            .collect()
    }

    /// A lost lease must never mark another worker's deletion as complete.
    pub async fn removed(&self, id: i64, epoch: i64) -> Result<bool, sqlx::Error> {
        let done = sqlx::query(
            "UPDATE cover_private_variant_removals
             SET state='removed',removed_at=now(),locked_until=NULL
             WHERE id=$1 AND lease_epoch=$2 AND state='leased'
               AND locked_until>now()",
        )
        .bind(id)
        .bind(epoch)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() == 1)
    }

    pub async fn retry(&self, id: i64, epoch: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE cover_private_variant_removals
             SET state='pending',locked_until=NULL,
                 retry_after=now()+interval '10 minutes'
             WHERE id=$1 AND lease_epoch=$2 AND state='leased'
               AND locked_until>now()",
        )
        .bind(id)
        .bind(epoch)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }
}
