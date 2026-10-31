//! Consume C's verified view, never arbitrary browser claims or naked outbox payloads.
use super::*;
use sqlx::Row;

const CONSUMER: &str = "recommendation-feedback-v1";

impl PgRecommendations {
    pub async fn pending_read_events(&self) -> Result<Vec<String>, FeedbackError> {
        Ok(sqlx::query_scalar(
            "SELECT v.event_id::text FROM rec_verified_read_events_v2 v
             WHERE v.occurred_at>now()-interval '30 days'
               AND NOT EXISTS(SELECT 1 FROM event_deliveries d WHERE d.event_id=v.event_id AND d.consumer=$1)
             ORDER BY v.occurred_at,v.event_id LIMIT 64")
            .bind(CONSUMER).fetch_all(&self.pool).await?)
    }

    /// Receipt and all F effects commit together. The authoritative C fact is untouched.
    pub async fn accept_read_event(&self, event: &str) -> Result<bool, FeedbackError> {
        self.accept_read_event_with_policy(event, true).await
    }
    pub(super) async fn accept_read_event_with_policy(
        &self,
        event: &str,
        collect: bool,
    ) -> Result<bool, FeedbackError> {
        if event.len() != 36 || !event.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
            return Err(FeedbackError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        let exists: Option<String> =
            sqlx::query_scalar("SELECT id::text FROM outbox_events WHERE id=$1::uuid FOR UPDATE")
                .bind(event)
                .fetch_optional(&mut *tx)
                .await?;
        if exists.is_none() {
            return Err(FeedbackError::NotFound);
        }
        let replay: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM event_deliveries WHERE consumer=$1 AND event_id=$2::uuid)",
        )
        .bind(CONSUMER)
        .bind(event)
        .fetch_one(&mut *tx)
        .await?;
        if replay {
            return Ok(false);
        }
        let evidence=sqlx::query("SELECT user_id,book_id,kind,first_for_book FROM rec_verified_read_events_v2 WHERE event_id=$1::uuid AND occurred_at<=now()")
            .bind(event).fetch_optional(&mut *tx).await?.ok_or(FeedbackError::Forbidden)?;
        let actor: i64 = evidence.try_get("user_id")?;
        let book: i64 = evidence.try_get("book_id")?;
        let kind: String = evidence.try_get("kind")?;
        let first: bool = evidence.try_get("first_for_book")?;
        let subject: Option<String>=sqlx::query_scalar(
            "SELECT s.subject_id::text FROM rec_subjects s
             WHERE s.user_id=$1 AND s.enabled
               AND EXISTS(SELECT 1 FROM rec_verified_read_events_v2 WHERE event_id=$2::uuid AND occurred_at>=s.enabled_at AND occurred_at>now()-interval '30 days')
             FOR UPDATE")
            .bind(actor).bind(event).fetch_optional(&mut *tx).await?;
        if let Some(subject) = subject.filter(|_| collect) {
            let allowed: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rec_public_books WHERE id=$1)")
                    .bind(book)
                    .fetch_one(&mut *tx)
                    .await?;
            if allowed {
                // Even a later chapter is evidence against an inferred abandonment;
                // only first_for_book can create a new-reader conversion.
                sqlx::query("UPDATE rec_assignments a SET progress_observed_at=v.occurred_at FROM rec_verified_read_events_v2 v WHERE v.event_id=$1::uuid AND a.subject_id=$2::uuid AND a.book_id=v.book_id AND a.clicked_at<=v.occurred_at AND a.clicked_at>=v.occurred_at-interval '14 days' AND a.progress_observed_at IS NULL")
                    .bind(event).bind(&subject).execute(&mut *tx).await?;
                if kind == "chapter_finished" {
                    sqlx::query("INSERT INTO rec_reader_signals(subject_id,book_id,occurred_at,source_event_id) SELECT $1::uuid,book_id,occurred_at,event_id FROM rec_verified_read_events_v2 WHERE event_id=$2::uuid ON CONFLICT(subject_id,book_id) DO NOTHING")
                        .bind(&subject).bind(event).execute(&mut *tx).await?;
                }
                if first {
                    let days: i32 = if kind == "chapter_started" { 7 } else { 14 };
                    let assignment: Option<String>=sqlx::query_scalar(
                        "SELECT a.id::text FROM rec_assignments a JOIN rec_verified_read_events_v2 v ON v.event_id=$1::uuid
                         WHERE a.subject_id=$2::uuid AND a.book_id=v.book_id AND a.clicked_at<=v.occurred_at
                           AND a.clicked_at>=v.occurred_at-make_interval(days=>$3::int)
                         ORDER BY a.clicked_at DESC,a.id ASC LIMIT 1 FOR UPDATE OF a")
                        .bind(event).bind(&subject).bind(days).fetch_optional(&mut *tx).await?;
                    if let Some(assignment) = assignment {
                        let inserted=sqlx::query("INSERT INTO rec_first_conversions(subject_id,book_id,kind,assignment_id,source_event_id,occurred_at) SELECT $1::uuid,book_id,kind,$2::uuid,event_id,occurred_at FROM rec_verified_read_events_v2 WHERE event_id=$3::uuid ON CONFLICT DO NOTHING")
                            .bind(&subject).bind(&assignment).bind(event).execute(&mut *tx).await?;
                        if inserted.rows_affected() == 1 {
                            sqlx::query("UPDATE rec_assignments a SET started_at=CASE WHEN v.kind='chapter_started' THEN v.occurred_at ELSE a.started_at END,completed_at=CASE WHEN v.kind='chapter_finished' THEN v.occurred_at ELSE a.completed_at END FROM rec_verified_read_events_v2 v WHERE a.id=$1::uuid AND v.event_id=$2::uuid")
                                .bind(&assignment).bind(event).execute(&mut *tx).await?;
                            sqlx::query("INSERT INTO rec_attribution_events(assignment_id,kind,source_event_id,occurred_at,trust) SELECT $1::uuid,kind,event_id,occurred_at,'domain_committed' FROM rec_verified_read_events_v2 WHERE event_id=$2::uuid ON CONFLICT DO NOTHING")
                                .bind(&assignment).bind(event).execute(&mut *tx).await?;
                            dirty(&mut tx, &assignment).await?;
                        }
                    }
                }
            }
        }
        sqlx::query("INSERT INTO event_deliveries(consumer,event_id) VALUES($1,$2::uuid) ON CONFLICT DO NOTHING")
            .bind(CONSUMER).bind(event).execute(&mut *tx).await?;
        sqlx::query("UPDATE outbox_events e SET delivered_at=now() WHERE e.id=$1::uuid AND e.delivered_at IS NULL AND NOT EXISTS(SELECT 1 FROM unnest(e.required_consumers) AS wanted(consumer) WHERE NOT EXISTS(SELECT 1 FROM event_deliveries d WHERE d.event_id=e.id AND d.consumer=wanted.consumer))")
            .bind(event).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }
}
