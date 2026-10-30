//! F-12 / S046: private achievements from C's immutable completion facts.
//! Neither a view, an S3 grant, nor a client-supplied event can grant a badge.
#![forbid(unsafe_code)]

use crate::services::achievements::{AchievementDefinition, READER_ACHIEVEMENTS_V1, THRESHOLDS};
use sqlx::{PgPool, Row};
use std::collections::HashMap;

const CONSUMER: &str = "achievements-v1";
const BATCH_LIMIT: i64 = 32;

#[derive(Clone)]
pub struct PgAchievements {
    pool: PgPool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReaderBadge {
    pub definition: AchievementDefinition,
    pub unlocked_on: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReaderAchievements {
    pub completed_chapters: i64,
    pub badges: Vec<ReaderBadge>,
}

#[derive(Debug)]
pub enum AchievementStoreError {
    Invalid,
    Unavailable,
}

impl From<sqlx::Error> for AchievementStoreError {
    fn from(_: sqlx::Error) -> Self {
        Self::Unavailable
    }
}

impl PgAchievements {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// A private, read-only view: old C facts are covered by the SQL migration
    /// backfill; new grants are written exclusively by the trusted consumer.
    pub async fn snapshot(&self, actor: i64) -> Result<ReaderAchievements, AchievementStoreError> {
        if actor <= 0 {
            return Err(AchievementStoreError::Invalid);
        }
        let completed_chapters: i64 = sqlx::query_scalar(
            "SELECT count(*)::bigint FROM reader_chapter_completions WHERE user_id=$1",
        )
        .bind(actor)
        .fetch_one(&self.pool)
        .await?;
        let rows = sqlx::query(
            "SELECT achievement_key,
                    to_char(unlocked_at AT TIME ZONE 'UTC', 'YYYY-MM-DD') AS unlocked_on
             FROM user_achievement_grants
             WHERE user_id=$1 AND catalog_version=1",
        )
        .bind(actor)
        .fetch_all(&self.pool)
        .await?;
        let mut unlocked = HashMap::with_capacity(rows.len());
        for row in rows {
            unlocked.insert(
                row.try_get::<String, _>("achievement_key")?,
                row.try_get::<String, _>("unlocked_on")?,
            );
        }
        Ok(ReaderAchievements {
            completed_chapters,
            badges: READER_ACHIEVEMENTS_V1
                .iter()
                .copied()
                .map(|definition| ReaderBadge {
                    unlocked_on: unlocked.remove(definition.key),
                    definition,
                })
                .collect(),
        })
    }

    /// Bounded polling: no second queue, and never claim other consumers' work.
    pub async fn pending_completion_event_ids(
        &self,
    ) -> Result<Vec<String>, AchievementStoreError> {
        Ok(sqlx::query_scalar(
            "SELECT e.id::text
             FROM outbox_events AS e
             JOIN reader_chapter_completions AS c ON c.source_event_id=e.id
             WHERE e.event_type='ChapterCompletedV1' AND e.schema_version=1
               AND $1=ANY(e.required_consumers) AND e.delivered_at IS NULL
               AND (e.lease_until IS NULL OR e.lease_until<now())
               AND NOT EXISTS (
                   SELECT 1 FROM event_deliveries AS d
                   WHERE d.consumer=$1 AND d.event_id=e.id
               )
             ORDER BY e.created_at,e.id LIMIT $2",
        )
        .bind(CONSUMER)
        .bind(BATCH_LIMIT)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Atomically materialize every crossed threshold and acknowledge this
    /// verified C event. Replays and simultaneous workers cannot grant twice.
    pub async fn accept_completed_chapter(
        &self,
        event_id: &str,
    ) -> Result<bool, AchievementStoreError> {
        if event_id.len() != 36
            || !event_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
        {
            return Err(AchievementStoreError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        // Require the exact C-owned completion with its version and matching
        // paid-purchase provenance; an arbitrary outbox payload is never proof.
        let source = sqlx::query(
            "SELECT c.user_id
             FROM outbox_events AS e
             JOIN reader_chapter_completions AS c ON c.source_event_id=e.id
             JOIN chapters AS ch ON ch.id=c.chapter_id AND ch.book_id=c.book_id
             JOIN chapter_versions AS v
               ON v.id=c.first_version_id AND v.chapter_id=c.chapter_id
             WHERE e.id=$1::uuid AND e.event_type='ChapterCompletedV1'
               AND e.schema_version=1 AND e.aggregate_kind='chapter'
               AND e.aggregate_id=c.chapter_id
               AND e.payload->>'user_id'=c.user_id::text
               AND e.payload->>'book_id'=c.book_id::text
               AND e.payload->>'chapter_id'=c.chapter_id::text
               AND e.payload->>'version_id'=c.first_version_id::text
               AND (
                   (c.purchase_id IS NULL
                    AND e.payload->>'authority'='C-verified-free-position'
                    AND NOT(e.payload ? 'purchase_id'))
                   OR (c.purchase_id IS NOT NULL
                    AND e.payload->>'authority'='C-verified-purchased-position'
                    AND e.payload->>'purchase_id'=c.purchase_id::text
                    AND EXISTS (
                        SELECT 1 FROM chapter_purchase_facts AS p
                        WHERE p.id=c.purchase_id AND p.user_id=c.user_id
                          AND p.chapter_id=c.chapter_id
                          AND p.version_id=c.first_version_id
                    ))
               )
               AND $2=ANY(e.required_consumers)
             FOR UPDATE OF e",
        )
        .bind(event_id)
        .bind(CONSUMER)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AchievementStoreError::Invalid)?;
        let user_id: i64 = source.try_get("user_id")?;
        let already_delivered: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM event_deliveries
                WHERE consumer=$1 AND event_id=$2::uuid
             )",
        )
        .bind(CONSUMER)
        .bind(event_id)
        .fetch_one(&mut *tx)
        .await?;
        if already_delivered {
            tx.commit().await?;
            return Ok(false);
        }

        // The Nth trusted completion is the evidence and unlock timestamp.
        // Do not increment an untrusted counter: recompute from C's unique
        // (user_id,book_id,chapter_id) canonical facts for this one user.
        sqlx::query(
            "INSERT INTO user_achievement_grants
                (user_id,achievement_key,catalog_version,unlocked_at,source_event_id)
             SELECT $1, 'reader_chapters_' || numbered.ordinal::text,
                    1, numbered.completed_at, numbered.source_event_id
             FROM (
                 SELECT completed_at, chapter_id, source_event_id,
                     row_number() OVER (ORDER BY completed_at,chapter_id) AS ordinal
                 FROM reader_chapter_completions WHERE user_id=$1
             ) AS numbered
             WHERE numbered.ordinal = ANY($2::bigint[])
             ON CONFLICT(user_id,achievement_key) DO NOTHING",
        )
        .bind(user_id)
        .bind(THRESHOLDS.to_vec())
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO event_deliveries(consumer,event_id)
             VALUES($1,$2::uuid) ON CONFLICT(consumer,event_id) DO NOTHING",
        )
        .bind(CONSUMER)
        .bind(event_id)
        .execute(&mut *tx)
        .await?;
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
