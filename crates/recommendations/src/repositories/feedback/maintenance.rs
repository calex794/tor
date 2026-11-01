//! Bounded restart-safe materialization. No counter increments without a receipt.
use super::*;
use sqlx::Row;

impl PgRecommendations {
    pub async fn refresh_quality(&self) -> Result<usize, FeedbackError> {
        let mut tx = self.pool.begin().await?;
        let rows=sqlx::query("DELETE FROM rec_dirty_buckets WHERE (bucket_date,book_id) IN (SELECT bucket_date,book_id FROM rec_dirty_buckets ORDER BY bucket_date,book_id LIMIT 32 FOR UPDATE SKIP LOCKED) RETURNING bucket_date::text,book_id")
            .fetch_all(&mut *tx).await?;
        for row in &rows {
            let date: String = row.try_get("bucket_date")?;
            let book: i64 = row.try_get("book_id")?;
            // The row lock blocks a concurrent dirty insertion until this transaction
            // commits; the next change requeues rather than being lost.
            sqlx::query("DELETE FROM rec_quality_daily WHERE bucket_date=$1::date AND book_id=$2")
                .bind(&date)
                .bind(book)
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                "INSERT INTO rec_quality_daily(bucket_date,book_id,surface,source,reason,position,generation,algorithm_version,impressions,paired_clicks,unpaired_clicks,starts,completions)
                 SELECT cohort_day,book_id,surface,source,reason,position,COALESCE(generation_id::text,'fallback'),algorithm_version,
                   count(*) FILTER(WHERE viewed_at IS NOT NULL),
                   count(*) FILTER(WHERE viewed_at IS NOT NULL AND clicked_at IS NOT NULL),
                   count(*) FILTER(WHERE viewed_at IS NULL AND clicked_at IS NOT NULL),
                   count(*) FILTER(WHERE started_at IS NOT NULL),count(*) FILTER(WHERE completed_at IS NOT NULL)
                 FROM rec_assignments WHERE cohort_day=$1::date AND book_id=$2
                 GROUP BY cohort_day,book_id,surface,source,reason,position,generation_id,algorithm_version")
                .bind(&date).bind(book).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(rows.len())
    }

    /// Called by the G-owned worker with the application pool, never from an HTTP GET.
    pub async fn tick_feedback(
        &self,
        collect: bool,
        collect_organic: bool,
    ) -> Result<usize, FeedbackError> {
        let pending = self.pending_read_events().await?;
        for event in &pending {
            self.accept_read_event_with_policy(event, collect, collect && collect_organic)
                .await?;
        }
        self.refresh_quality().await?;
        self.refresh_organic_quality().await?;
        if self.pending_read_events().await?.is_empty() {
            sqlx::query("INSERT INTO rec_worker_health(singleton,last_success) VALUES(true,now()) ON CONFLICT(singleton) DO UPDATE SET last_success=EXCLUDED.last_success")
                .execute(&self.pool).await?;
        }
        Ok(pending.len())
    }

    /// Raw personal data expires; anonymous historical aggregates are retained
    /// separately. Opt-out, unlike TTL, also recomputes unexpired cohorts.
    pub async fn prune(&self) -> Result<u64, FeedbackError> {
        let mut tx = self.pool.begin().await?;
        let mut count = 0;
        for statement in [
            "DELETE FROM rec_assignments WHERE id IN(SELECT id FROM rec_assignments WHERE created_at<now()-interval '30 days' ORDER BY created_at LIMIT 1000)",
            "DELETE FROM organic_discovery_touches WHERE id IN(SELECT id FROM organic_discovery_touches WHERE created_at<now()-interval '30 days' ORDER BY created_at LIMIT 1000)",
            "DELETE FROM organic_discovery_unattributed_reads WHERE source_event_id IN(SELECT source_event_id FROM organic_discovery_unattributed_reads WHERE occurred_at<now()-interval '30 days' ORDER BY occurred_at LIMIT 1000)",
            "DELETE FROM rec_reader_signals WHERE source_event_id IN(SELECT source_event_id FROM rec_reader_signals WHERE occurred_at<now()-interval '30 days' ORDER BY occurred_at LIMIT 1000)",
            "DELETE FROM rec_preference_commands WHERE (user_id,command_key) IN(SELECT user_id,command_key FROM rec_preference_commands WHERE created_at<now()-interval '30 days' ORDER BY created_at LIMIT 1000)",
            "DELETE FROM rec_quality_daily WHERE bucket_date<(now() AT TIME ZONE 'UTC')::date-180",
            "DELETE FROM organic_discovery_quality_daily WHERE bucket_date<(now() AT TIME ZONE 'UTC')::date-180",
        ] {
            count+=sqlx::query(statement).execute(&mut *tx).await?.rows_affected();
        }
        // Serialize retention erasure with feedback writes through the subject
        // row. After deleting per-book history, advance an anonymous CAS epoch
        // so even an old revision-zero request cannot recreate a hidden book.
        sqlx::query(
            "WITH locked_subjects AS (
                SELECT s.subject_id FROM rec_subjects s
                WHERE EXISTS (
                    SELECT 1 FROM rec_feedback_state f
                    WHERE f.subject_id=s.subject_id
                      AND f.expires_at<now()-interval '1 hour'
                )
                ORDER BY s.subject_id LIMIT 32 FOR UPDATE OF s SKIP LOCKED
             ), erased AS (
                DELETE FROM rec_feedback_state f USING locked_subjects s
                WHERE f.subject_id=s.subject_id
                  AND f.expires_at<now()-interval '1 hour'
                RETURNING f.subject_id
             )
             UPDATE rec_subjects s
             SET feedback_epoch=feedback_epoch+1
             WHERE s.subject_id IN (SELECT DISTINCT subject_id FROM erased)",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(count)
    }
}
