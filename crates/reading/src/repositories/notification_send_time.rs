//! Read-only S018 reader-owned habit estimate from C verified start events.
//! This is not an external address, channel consent, timer or delivery attempt.
#![forbid(unsafe_code)]
use crate::repositories::notification_preferences::PreferenceError;
use crate::services::notification_send_time::{choose_preferred_hour, PreferredSendHour};
use sqlx::PgPool;

#[derive(Clone)]
pub struct PgPreferredSendHour {
    pool: PgPool,
}

impl PgPreferredSendHour {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// At most 500 recent canonical first-start facts, verified against their
    /// matching outbox authority. No title, chapter, user ID or per-event time
    /// is returned to the caller; nothing is persisted beyond existing C facts.
    pub async fn estimate(&self, actor: i64) -> Result<PreferredSendHour, PreferenceError> {
        if actor <= 0 {
            return Err(PreferenceError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("SET TRANSACTION READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        sqlx::query("SET LOCAL statement_timeout = '750ms'")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        let recent: Vec<i32> = sqlx::query_scalar(
            r#"SELECT EXTRACT(HOUR FROM s.started_at AT TIME ZONE 'UTC')::int
               FROM reader_chapter_starts s
               JOIN users reader ON reader.id=s.user_id AND reader.account_state='active'
               JOIN outbox_events e ON e.id=s.source_event_id
               WHERE s.user_id=$1
                 AND s.started_at>=transaction_timestamp()-interval '30 days'
                 AND s.started_at<transaction_timestamp()
                 AND e.event_type='ChapterStartedV1' AND e.schema_version=1
                 AND e.aggregate_kind='chapter' AND e.aggregate_id=s.chapter_id
                 AND e.payload->>'user_id'=s.user_id::text
                 AND e.payload->>'book_id'=s.book_id::text
                 AND e.payload->>'chapter_id'=s.chapter_id::text
                 AND e.payload->>'version_id'=s.first_version_id::text
                 AND (
                   (s.purchase_id IS NULL
                     AND e.payload->>'authority'='C-verified-free-start'
                     AND NOT(e.payload ? 'purchase_id'))
                   OR (s.purchase_id IS NOT NULL
                     AND e.payload->>'authority'='C-verified-purchased-start'
                     AND e.payload->>'purchase_id'=s.purchase_id::text)
                 )
               ORDER BY s.started_at DESC LIMIT 500"#,
        )
        .bind(actor)
        .fetch_all(&mut *tx)
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        let mut counters = [0_u32; 24];
        for hour in recent {
            let hour = usize::try_from(hour).map_err(|_| PreferenceError::Unavailable)?;
            let bucket = counters.get_mut(hour).ok_or(PreferenceError::Unavailable)?;
            *bucket += 1;
        }
        Ok(choose_preferred_hour(&counters))
    }
}

fn unavailable(_: sqlx::Error) -> PreferenceError {
    PreferenceError::Unavailable
}
