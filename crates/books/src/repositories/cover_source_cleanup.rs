//! B-owned retirement of PRIVATE original cover uploads shared by multiple jobs.
//! The source row is locked and transitioned to retired atomically with its
//! deletion ledger. All job, variant, moderation and slot records are retained.
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgPrivateCoverSourceRemovals {
    pool: PgPool,
}

pub struct PrivateSourceRemoval {
    pub id: i64,
    pub storage_key: String,
    pub lease_epoch: i64,
}

impl PgPrivateCoverSourceRemovals {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Claim source rows only if EVERY referenced job is terminal and its
    /// moderation request is rejected/superseded for at least seven days,
    /// EVERY attached private variant is confirmed removed, and NO job is a
    /// current public Book cover. Unknown/legacy/pending requests fail closed.
    ///
    /// FOR UPDATE OF s SKIP LOCKED coordinates with PgCoverJobs::enqueue's
    /// FOR SHARE OF s: no new recipe can be enqueued while a source retires.
    pub async fn stage_eligible(&self, limit: i64) -> Result<u64, sqlx::Error> {
        if !(1..=12).contains(&limit) {
            return Err(sqlx::Error::Protocol(
                "invalid cover source retirement batch".into(),
            ));
        }
        let result = sqlx::query(
            "WITH eligible AS (
                SELECT s.id
                  FROM cover_upload_objects s
                 WHERE s.status='verified'
                   AND s.storage_key LIKE 'staging/covers/sources/%'
                   AND NOT EXISTS (
                       SELECT 1 FROM cover_private_source_removals existing
                       WHERE existing.source_object_id=s.id
                   )
                   AND EXISTS (
                       SELECT 1 FROM cover_variant_jobs any_job
                       WHERE any_job.source_object_id=s.id
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM cover_variant_jobs j
                       WHERE j.source_object_id=s.id
                         AND (
                           j.status NOT IN ('done','superseded','dead')
                           OR NOT EXISTS (
                              SELECT 1 FROM book_cover_review_requests r
                              WHERE r.job_id=j.id
                                AND (
                                  (r.state='rejected' AND
                                   r.decided_at<=now()-interval '7 days')
                                  OR (r.state='superseded' AND
                                   r.superseded_at<=now()-interval '7 days')
                                )
                           )
                         )
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM book_cover_slots slot
                       JOIN cover_variant_jobs j ON j.id=slot.job_id
                       WHERE j.source_object_id=s.id
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM cover_variant_jobs j
                       JOIN cover_variants v ON v.job_id=j.id
                       LEFT JOIN cover_private_variant_removals done
                         ON done.job_id=v.job_id
                        AND done.variant_name=v.variant_name
                        AND done.storage_key=v.storage_key
                       WHERE j.source_object_id=s.id
                         AND (done.id IS NULL OR done.state<>'removed')
                   )
                 ORDER BY s.id
                 FOR UPDATE OF s SKIP LOCKED LIMIT $1
            ), retired AS (
                UPDATE cover_upload_objects s SET status='retired'
                  FROM eligible e
                 WHERE e.id=s.id AND s.status='verified'
                 RETURNING s.id,s.storage_key
            )
            INSERT INTO cover_private_source_removals
                (source_object_id,storage_key)
            SELECT r.id,r.storage_key FROM retired r WHERE true
            ON CONFLICT DO NOTHING",
        )
        .bind(limit)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Recheck the entire source sharing graph after claiming a fenced lease.
    /// No pending recipe, approved Book slot or unremoved variant may be used
    /// to delete the original. The retired status prevents new recipe creation.
    pub async fn claim(&self, limit: i64) -> Result<Vec<PrivateSourceRemoval>, sqlx::Error> {
        if !(1..=2).contains(&limit) {
            return Err(sqlx::Error::Protocol(
                "invalid private source deletion claim size".into(),
            ));
        }
        let rows = sqlx::query(
            "WITH eligible AS (
                SELECT c.id FROM cover_private_source_removals c
                  JOIN cover_upload_objects s
                    ON s.id=c.source_object_id AND s.storage_key=c.storage_key
                 WHERE s.status='retired'
                   AND NOT EXISTS (
                       SELECT 1 FROM cover_variant_jobs j
                       WHERE j.source_object_id=s.id
                         AND (
                           j.status NOT IN ('done','superseded','dead')
                           OR NOT EXISTS (
                              SELECT 1 FROM book_cover_review_requests r
                              WHERE r.job_id=j.id
                                AND (
                                  (r.state='rejected' AND
                                   r.decided_at<=now()-interval '7 days')
                                  OR (r.state='superseded' AND
                                   r.superseded_at<=now()-interval '7 days')
                                )
                           )
                         )
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM book_cover_slots slot
                       JOIN cover_variant_jobs j ON j.id=slot.job_id
                       WHERE j.source_object_id=s.id
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM cover_variant_jobs j
                       JOIN cover_variants v ON v.job_id=j.id
                       LEFT JOIN cover_private_variant_removals done
                         ON done.job_id=v.job_id
                        AND done.variant_name=v.variant_name
                        AND done.storage_key=v.storage_key
                       WHERE j.source_object_id=s.id
                         AND (done.id IS NULL OR done.state<>'removed')
                   )
                   AND (
                     (c.state='pending' AND c.retry_after<=now())
                     OR (c.state='leased' AND c.locked_until<now())
                   )
                 ORDER BY c.id
                 FOR UPDATE OF c SKIP LOCKED LIMIT $1
            )
            UPDATE cover_private_source_removals c
               SET state='leased',lease_epoch=c.lease_epoch+1,
                   locked_until=now()+interval '60 seconds'
              FROM eligible e WHERE e.id=c.id
            RETURNING c.id,c.storage_key,c.lease_epoch",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(PrivateSourceRemoval {
                    id: row.try_get("id")?,
                    storage_key: row.try_get("storage_key")?,
                    lease_epoch: row.try_get("lease_epoch")?,
                })
            })
            .collect()
    }

    pub async fn removed(&self, id: i64, epoch: i64) -> Result<bool, sqlx::Error> {
        let done = sqlx::query(
            "UPDATE cover_private_source_removals
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
        let done = sqlx::query(
            "UPDATE cover_private_source_removals
                SET state='pending',locked_until=NULL,
                    retry_after=now()+interval '10 minutes'
              WHERE id=$1 AND lease_epoch=$2 AND state='leased'
                AND locked_until>now()",
        )
        .bind(id)
        .bind(epoch)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() == 1)
    }
}
