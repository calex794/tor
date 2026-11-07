//! Operator-only S025 CDN state counters. No IDs, paths, tokens or user
//! data leave this read-only aggregate query; missing schema is unknown.
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgCoverCdnOps {
    pool: PgPool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CdnQueueCounts {
    pub reserved: i64,
    pub copied: i64,
    pub removal_due: i64,
    pub origin_deleted: i64,
    pub purge_submitted: i64,
    pub purged: i64,
    pub stale_reserved: i64,
    pub overdue_removal: i64,
    pub overdue_purge: i64,
    pub repeated_attempts: i64,
}

impl PgCoverCdnOps {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Not having the S025 ledger is an error, never an empty queue.
    pub async fn queue_counts(&self) -> Result<CdnQueueCounts, sqlx::Error> {
        let row = sqlx::query(
            "SELECT
                COUNT(*) FILTER (WHERE state='reserved') AS reserved,
                COUNT(*) FILTER (WHERE state='copied') AS copied,
                COUNT(*) FILTER (WHERE state='removal_due') AS removal_due,
                COUNT(*) FILTER (WHERE state='origin_deleted') AS origin_deleted,
                COUNT(*) FILTER (WHERE state='purge_submitted') AS purge_submitted,
                COUNT(*) FILTER (WHERE state='purged') AS purged,
                COUNT(*) FILTER (
                    WHERE state='reserved'
                      AND updated_at < now()-interval '15 minutes'
                ) AS stale_reserved,
                COUNT(*) FILTER (
                    WHERE state='removal_due'
                      AND updated_at < now()-interval '5 minutes'
                ) AS overdue_removal,
                COUNT(*) FILTER (
                    WHERE state IN ('origin_deleted','purge_submitted')
                      AND updated_at < now()-interval '20 minutes'
                ) AS overdue_purge,
                COUNT(*) FILTER (
                    WHERE state NOT IN ('copied','purged') AND attempts >= 3
                ) AS repeated_attempts
             FROM public_cover_publications",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(CdnQueueCounts {
            reserved: row.try_get("reserved")?,
            copied: row.try_get("copied")?,
            removal_due: row.try_get("removal_due")?,
            origin_deleted: row.try_get("origin_deleted")?,
            purge_submitted: row.try_get("purge_submitted")?,
            purged: row.try_get("purged")?,
            stale_reserved: row.try_get("stale_reserved")?,
            overdue_removal: row.try_get("overdue_removal")?,
            overdue_purge: row.try_get("overdue_purge")?,
            repeated_attempts: row.try_get("repeated_attempts")?,
        })
    }
}
