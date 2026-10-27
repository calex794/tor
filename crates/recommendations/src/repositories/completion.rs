//! F-05: consume only C-owned, durable free or D-checked pinned paid V1 facts.
//! A GET, bookmark, unverified client flag or arbitrary outbox payload is
//! never allowed to start a personal placement.
use super::postgres::{load_snapshot, persist, PgRanking, RankingStoreError};
use crate::services::ranking::MAX_RANKED_BOOKS;
use sqlx::Row;

const CONSUMER: &str = "ranking-eligibility-v1";
const BATCH_LIMIT: i64 = 32;

impl PgRanking {
    /// A bounded poll of C events. Readers already comparing another book
    /// are excluded; their events remain durable and will be retried later.
    pub async fn pending_completion_event_ids(&self) -> Result<Vec<String>, RankingStoreError> {
        sqlx::query_scalar(
            "SELECT e.id::text FROM outbox_events AS e
             JOIN reader_chapter_completions AS c ON c.source_event_id=e.id
             JOIN chapters AS ch ON ch.id=c.chapter_id AND ch.book_id=c.book_id
             JOIN books AS b ON b.id=c.book_id
             LEFT JOIN user_book_ranking_state AS active ON active.user_id=c.user_id
             WHERE e.event_type='ChapterCompletedV1' AND e.schema_version=1
               AND e.aggregate_kind='chapter' AND e.aggregate_id=c.chapter_id
               AND (
                   (c.purchase_id IS NULL
                       AND e.payload->>'authority'='C-verified-free-position'
                       AND NOT (e.payload ? 'purchase_id'))
                   OR (c.purchase_id IS NOT NULL
                       AND e.payload->>'authority'='C-verified-purchased-position'
                       AND e.payload->>'purchase_id'=c.purchase_id::text
                       AND EXISTS (
                           SELECT 1 FROM chapter_purchase_facts AS purchase
                           WHERE purchase.id=c.purchase_id
                             AND purchase.user_id=c.user_id
                             AND purchase.chapter_id=c.chapter_id
                             AND purchase.version_id=c.first_version_id
                       ))
               )
               AND e.payload->>'user_id'=c.user_id::text
               AND e.payload->>'book_id'=c.book_id::text
               AND e.payload->>'chapter_id'=c.chapter_id::text
               AND e.payload->>'version_id'=c.first_version_id::text
               AND $1=ANY(e.required_consumers)
               AND e.delivered_at IS NULL
               AND (e.lease_until IS NULL OR e.lease_until<now())
               AND active.user_id IS NULL
               AND b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
               AND NOT EXISTS (
                 SELECT 1 FROM event_deliveries AS d
                 WHERE d.event_id=e.id AND d.consumer=$1
               )
             ORDER BY e.created_at,e.id LIMIT $2",
        )
        .bind(CONSUMER)
        .bind(BATCH_LIMIT)
        .fetch_all(&self.pool)
        .await
        .map_err(Into::into)
    }

    /// One PostgreSQL transaction covers ranking mutation, consumer receipt,
    /// and the global delivered flag. Another process cannot double-apply it.
    /// Returns false when this event was previously consumed.
    pub async fn accept_completed_chapter(
        &self,
        event_id: &str,
    ) -> Result<bool, RankingStoreError> {
        if event_id.len() != 36 || !event_id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
            return Err(RankingStoreError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        // Lock source envelope and bind it to the independently committed C
        // completion record. A forged outbox payload alone is insufficient.
        let evidence = sqlx::query(
            "SELECT c.user_id,c.book_id
             FROM outbox_events AS e
             JOIN reader_chapter_completions AS c ON c.source_event_id=e.id
             JOIN chapters AS ch ON ch.id=c.chapter_id AND ch.book_id=c.book_id
             WHERE e.id=$1::uuid AND e.event_type='ChapterCompletedV1'
               AND e.schema_version=1 AND e.aggregate_kind='chapter'
               AND e.aggregate_id=c.chapter_id
               AND (
                   (c.purchase_id IS NULL
                       AND e.payload->>'authority'='C-verified-free-position'
                       AND NOT (e.payload ? 'purchase_id'))
                   OR (c.purchase_id IS NOT NULL
                       AND e.payload->>'authority'='C-verified-purchased-position'
                       AND e.payload->>'purchase_id'=c.purchase_id::text
                       AND EXISTS (
                           SELECT 1 FROM chapter_purchase_facts AS purchase
                           WHERE purchase.id=c.purchase_id
                             AND purchase.user_id=c.user_id
                             AND purchase.chapter_id=c.chapter_id
                             AND purchase.version_id=c.first_version_id
                       ))
               )
               AND e.payload->>'user_id'=c.user_id::text
               AND e.payload->>'book_id'=c.book_id::text
               AND e.payload->>'chapter_id'=c.chapter_id::text
               AND e.payload->>'version_id'=c.first_version_id::text
               AND $2=ANY(e.required_consumers)
             FOR UPDATE OF e",
        )
        .bind(event_id)
        .bind(CONSUMER)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(RankingStoreError::Invalid)?;
        let user_id: i64 = evidence.try_get("user_id")?;
        let book_id: i64 = evidence.try_get("book_id")?;
        let processed: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1 FROM event_deliveries
                 WHERE event_id=$1::uuid AND consumer=$2
             )",
        )
        .bind(event_id)
        .bind(CONSUMER)
        .fetch_one(&mut *tx)
        .await?;
        if processed {
            tx.commit().await?;
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO user_book_ranking_head(user_id,revision)
             SELECT id,0 FROM users WHERE id=$1
             ON CONFLICT(user_id) DO NOTHING",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
        let revision: i64 = sqlx::query_scalar(
            "SELECT revision FROM user_book_ranking_head WHERE user_id=$1 FOR UPDATE",
        )
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
        // A saved C fact never overrides current moderation or visibility.
        // Hold the row until the event/rank are committed.
        let allowed: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM books WHERE id=$1
             AND visibility='public' AND NOT admin_hold AND NOT is_deleted
             FOR SHARE",
        )
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await?;
        if allowed.is_none() {
            return Err(RankingStoreError::Conflict);
        }

        let before = load_snapshot(&mut tx, user_id, revision).await?;
        let mut next = before.clone();

        // A deferred book requires a genuinely NEW C completion after the
        // deferral. Replaying an older event cannot undo the reader's choice.
        let deferred_fresh: Option<bool> = sqlx::query_scalar(
            "SELECT c.completed_at>d.deferred_at
             FROM user_book_ranking_defer AS d
             JOIN reader_chapter_completions AS c ON c.user_id=d.user_id
               AND c.book_id=d.book_id AND c.source_event_id=$3::uuid
             WHERE d.user_id=$1 AND d.book_id=$2",
        )
        .bind(user_id)
        .bind(book_id)
        .bind(event_id)
        .fetch_optional(&mut *tx)
        .await?;
        if deferred_fresh == Some(true) {
            next = next.release_deferred_after_verified_read(book_id)?;
        }
        if deferred_fresh != Some(false) && next.ranks.len() < MAX_RANKED_BOOKS {
            // An active placement owned by a different completion causes a
            // retry, never a dropped event/ack. The poll excludes such users.
            let (started, _) = next.start_verified_placement(book_id)?;
            next = started;
        }
        if next != before {
            next.validate()?;
            persist(&mut tx, &before, &next, None).await?;
        }
        sqlx::query(
            "INSERT INTO event_deliveries(consumer,event_id)
             VALUES($1,$2::uuid) ON CONFLICT(consumer,event_id) DO NOTHING",
        )
        .bind(CONSUMER)
        .bind(event_id)
        .execute(&mut *tx)
        .await?;
        // Don't mark a multi-consumer event delivered until EVERY required
        // recipient has its own durable receipt.
        sqlx::query(
            "UPDATE outbox_events AS e SET delivered_at=now()
             WHERE e.id=$1::uuid AND e.delivered_at IS NULL
               AND NOT EXISTS (
                 SELECT 1 FROM unnest(e.required_consumers) AS wanted(consumer)
                 WHERE NOT EXISTS (
                   SELECT 1 FROM event_deliveries AS done
                   WHERE done.event_id=e.id AND done.consumer=wanted.consumer
                 )
               )",
        )
        .bind(event_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }
}
