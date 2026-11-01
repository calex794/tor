//! Source-verified organic discovery, distinct from recommendation assignments.
//! Every write requires an opted-in REC subject and a live session digest.
use super::*;
use crate::{
    domain::feedback::{book_id, token_digest, OrganicQualityRow, OrganicSource, MAX_ITEMS},
    services::feedback::{new_ticket, validate_actor},
};
use sqlx::Row;

impl PgRecommendations {
    /// Only IDs actually rendered in a successful public catalogue response
    /// are passed by the web middleware. Never store a raw search query.
    pub async fn issue_organic_tickets(
        &self,
        actor: i64,
        session: &[u8; 32],
        source: OrganicSource,
        ids: &[i64],
    ) -> Result<Vec<(i64, String)>, FeedbackError> {
        validate_actor(actor)?;
        if ids.is_empty() || ids.len() > MAX_ITEMS {
            return Err(FeedbackError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        let subject = lock_subject(&mut tx, actor).await?;
        let used: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM organic_discovery_touches
             WHERE subject_id=$1::uuid AND created_at>now()-interval '1 day'",
        )
        .bind(&subject)
        .fetch_one(&mut *tx)
        .await?;
        if used + ids.len() as i64 > 1000 {
            return Err(FeedbackError::Limited);
        }
        let mut issued = Vec::new();
        for (position, id) in ids.iter().copied().enumerate() {
            if book_id(&id.to_string())? != id {
                return Err(FeedbackError::Invalid);
            }
            if issued.iter().any(|(existing, _)| *existing == id) {
                continue;
            }
            let ticket = new_ticket()?;
            let digest = token_digest(&ticket)?;
            let inserted: Option<String> = sqlx::query_scalar(
                "INSERT INTO organic_discovery_touches
                 (subject_id,session_digest,ticket_digest,book_id,source,position)
                 SELECT $1::uuid,$2,$3,$4,$5,$6
                 WHERE EXISTS(SELECT 1 FROM rec_public_books WHERE id=$4)
                 RETURNING id::text",
            )
            .bind(&subject)
            .bind(session.as_slice())
            .bind(digest.as_slice())
            .bind(id)
            .bind(source.as_str())
            .bind((position + 1) as i16)
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(touch) = inserted {
                self::mark_dirty(&mut tx, &touch).await?;
                issued.push((id, ticket));
            }
        }
        tx.commit().await?;
        Ok(issued)
    }

    /// Called only by an authenticated first-party CSRF-protected POST.
    /// A ticket is never a read entitlement and cannot cross sessions.
    pub async fn activate_organic(
        &self,
        actor: i64,
        session: &[u8; 32],
        ticket: &str,
    ) -> Result<(), FeedbackError> {
        validate_actor(actor)?;
        let digest = token_digest(ticket)?;
        let mut tx = self.pool.begin().await?;
        let subject = lock_subject(&mut tx, actor).await?;
        let touch: Option<String> = sqlx::query_scalar(
            "UPDATE organic_discovery_touches t
             SET clicked_at=COALESCE(t.clicked_at,now())
             WHERE t.ticket_digest=$1 AND t.subject_id=$2::uuid AND t.session_digest=$3
               AND t.expires_at>now()
               AND EXISTS(SELECT 1 FROM rec_public_books WHERE id=t.book_id)
             RETURNING t.id::text",
        )
        .bind(digest.as_slice())
        .bind(&subject)
        .bind(session.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        let touch = touch.ok_or(FeedbackError::NotFound)?;
        mark_dirty(&mut tx, &touch).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Invoke after an actual successful 200 from the canonical book handler.
    /// Direct GETs without a previous real POST cannot create a book-open fact.
    pub async fn record_organic_book_open(
        &self,
        actor: i64,
        session: &[u8; 32],
        book: i64,
    ) -> Result<(), FeedbackError> {
        validate_actor(actor)?;
        validate_actor(book)?;
        let mut tx = self.pool.begin().await?;
        let subject = lock_subject(&mut tx, actor).await?;
        let touch: Option<String> = sqlx::query_scalar(
            "SELECT id::text FROM organic_discovery_touches
             WHERE subject_id=$1::uuid AND session_digest=$2 AND book_id=$3
               AND clicked_at>=now()-interval '15 minutes'
               AND EXISTS(SELECT 1 FROM rec_public_books WHERE id=$3)
             ORDER BY clicked_at DESC,id ASC LIMIT 1 FOR UPDATE",
        )
        .bind(&subject)
        .bind(session.as_slice())
        .bind(book)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(touch) = touch {
            sqlx::query(
                "UPDATE organic_discovery_touches
                 SET book_opened_at=COALESCE(book_opened_at,now()) WHERE id=$1::uuid",
            )
            .bind(&touch)
            .execute(&mut *tx)
            .await?;
            mark_dirty(&mut tx, &touch).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Operator-only aggregates: issued result links are not view impressions.
    pub async fn organic_quality(
        &self,
        days: i32,
    ) -> Result<Vec<OrganicQualityRow>, FeedbackError> {
        if !(1..=180).contains(&days) {
            return Err(FeedbackError::Invalid);
        }
        let rows = sqlx::query(
            "SELECT q.bucket_date::text,q.book_id::text,q.source,
                    q.issued,q.clicks,q.book_opens,q.starts,q.completions
             FROM organic_discovery_quality_daily q JOIN rec_public_books b ON b.id=q.book_id
             WHERE q.bucket_date>=(now() AT TIME ZONE 'UTC')::date-$1
             ORDER BY q.bucket_date DESC,q.book_id,q.source LIMIT 100",
        )
        .bind(days)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(OrganicQualityRow {
                    day: r.try_get("bucket_date")?,
                    book_id: r.try_get("book_id")?,
                    source: r.try_get("source")?,
                    issued: r.try_get("issued")?,
                    clicks: r.try_get("clicks")?,
                    book_opens: r.try_get("book_opens")?,
                    starts: r.try_get("starts")?,
                    completions: r.try_get("completions")?,
                })
            })
            .collect()
    }

    /// Rebuild counts from canonical per-ticket state, never increment blindly.
    pub async fn refresh_organic_quality(&self) -> Result<usize, FeedbackError> {
        let mut tx = self.pool.begin().await?;
        let rows = sqlx::query(
            "DELETE FROM organic_discovery_dirty
             WHERE (bucket_date,book_id) IN (
                 SELECT bucket_date,book_id FROM organic_discovery_dirty
                 ORDER BY bucket_date,book_id LIMIT 32 FOR UPDATE SKIP LOCKED
             ) RETURNING bucket_date::text,book_id",
        )
        .fetch_all(&mut *tx)
        .await?;
        for row in &rows {
            let day: String = row.try_get("bucket_date")?;
            let book: i64 = row.try_get("book_id")?;
            sqlx::query(
                "DELETE FROM organic_discovery_quality_daily
                 WHERE bucket_date=$1::date AND book_id=$2",
            )
            .bind(&day)
            .bind(book)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO organic_discovery_quality_daily
                 (bucket_date,book_id,source,issued,clicks,book_opens,starts,completions)
                 SELECT cohort_day,book_id,source,count(*),
                    count(*) FILTER(WHERE clicked_at IS NOT NULL),
                    count(*) FILTER(WHERE book_opened_at IS NOT NULL),
                    count(*) FILTER(WHERE started_at IS NOT NULL),
                    count(*) FILTER(WHERE completed_at IS NOT NULL)
                 FROM organic_discovery_touches WHERE cohort_day=$1::date AND book_id=$2
                 GROUP BY cohort_day,book_id,source",
            )
            .bind(&day)
            .bind(book)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(rows.len())
    }
}

pub(super) async fn mark_dirty(
    tx: &mut Transaction<'_, Postgres>,
    touch: &str,
) -> Result<(), FeedbackError> {
    sqlx::query(
        "INSERT INTO organic_discovery_dirty(bucket_date,book_id)
         SELECT cohort_day,book_id FROM organic_discovery_touches WHERE id=$1::uuid
         ON CONFLICT DO NOTHING",
    )
    .bind(touch)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
