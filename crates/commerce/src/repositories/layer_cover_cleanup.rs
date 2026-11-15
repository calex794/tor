//! Durable, opt-in cleanup of retired private package image variants.
//! Uploaded source images are never retained; public approval is never deleted.
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgPackageCoverVariantRemovals {
    pool: PgPool,
}

pub struct PackageCoverRemoval {
    pub id: i64,
    pub candidate_id: i64,
    pub storage_key: String,
    pub lease_epoch: i64,
}

impl PgPackageCoverVariantRemovals {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn stage_eligible(&self, limit: i64) -> Result<u64, sqlx::Error> {
        if !(1..=20).contains(&limit) {
            return Err(sqlx::Error::Protocol(
                "invalid package cleanup batch".into(),
            ));
        }
        // A crashed upload stops blocking a new upload after 30 minutes,
        // but its private data is not eligible for cleanup for seven more days.
        sqlx::query(
            "WITH old AS (
                SELECT id FROM commerce_layer_cover_candidates
                WHERE status='uploading'
                  AND submitted_at<now()-interval '30 minutes'
                ORDER BY id FOR UPDATE SKIP LOCKED LIMIT $1
             )
             UPDATE commerce_layer_cover_candidates c
             SET status='failed',terminal_at=now()
             FROM old WHERE c.id=old.id",
        )
        .bind(limit)
        .execute(&self.pool)
        .await?;
        let result = sqlx::query(
            "INSERT INTO commerce_layer_cover_variant_removals
                 (candidate_id,variant_name,storage_key)
             SELECT c.id,v.kind,CASE WHEN v.kind='main' THEN c.main_key ELSE c.card_key END
             FROM commerce_layer_cover_candidates c
             CROSS JOIN (VALUES ('main'),('card')) AS v(kind)
             LEFT JOIN commerce_layer_cover_variant_removals prior
               ON prior.candidate_id=c.id AND prior.variant_name=v.kind
             WHERE prior.id IS NULL
               AND c.status IN ('failed','rejected','superseded')
               AND c.terminal_at<=now()-interval '7 days'
             ORDER BY c.id,v.kind
             LIMIT $1 ON CONFLICT DO NOTHING",
        )
        .bind(limit)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Fenced lease: stale workers cannot finish/retry a newer claim.
    pub async fn claim(&self, limit: i64) -> Result<Vec<PackageCoverRemoval>, sqlx::Error> {
        if !(1..=3).contains(&limit) {
            return Err(sqlx::Error::Protocol(
                "invalid package cleanup lease count".into(),
            ));
        }
        let rows = sqlx::query(
            "WITH eligible AS (
                SELECT r.id FROM commerce_layer_cover_variant_removals r
                JOIN commerce_layer_cover_candidates c ON c.id=r.candidate_id
                WHERE c.status IN ('failed','rejected','superseded')
                  AND c.terminal_at<=now()-interval '7 days'
                  AND ((r.state='pending' AND r.retry_after<=now())
                       OR (r.state='leased' AND r.locked_until<now()))
                ORDER BY r.id FOR UPDATE OF r SKIP LOCKED LIMIT $1
             )
             UPDATE commerce_layer_cover_variant_removals r
             SET state='leased',lease_epoch=r.lease_epoch+1,
                 locked_until=now()+interval '60 seconds'
             FROM eligible e WHERE r.id=e.id
             RETURNING r.id,r.candidate_id,r.storage_key,r.lease_epoch",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(PackageCoverRemoval {
                    id: row.try_get("id")?,
                    candidate_id: row.try_get("candidate_id")?,
                    storage_key: row.try_get("storage_key")?,
                    lease_epoch: row.try_get("lease_epoch")?,
                })
            })
            .collect()
    }

    pub async fn removed(&self, id: i64, epoch: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE commerce_layer_cover_variant_removals
             SET state='removed',removed_at=now(),locked_until=NULL
             WHERE id=$1 AND lease_epoch=$2 AND state='leased'
               AND locked_until>now()",
        )
        .bind(id)
        .bind(epoch)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn retry(&self, id: i64, epoch: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE commerce_layer_cover_variant_removals
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
