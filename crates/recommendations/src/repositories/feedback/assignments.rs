use super::*;
use crate::domain::feedback::{book_id, token_digest, Feed, Surface, MAX_ITEMS};
use crate::services::feedback::{new_ticket, validate_actor};
use sqlx::Row;

impl PgRecommendations {
    /// Session-bound, opaque server assignments; a token never grants reading rights.
    pub async fn issue_tickets(&self, actor: i64, session: &[u8;32], surface: Surface, mut feed: Feed) -> Result<Feed, FeedbackError> {
        validate_actor(actor)?;
        if feed.items.len()>MAX_ITEMS { return Err(FeedbackError::Invalid); }
        if feed.items.is_empty() { return Ok(feed); }
        let mut tx = self.pool.begin().await?;
        let subject = lock_subject(&mut tx,actor).await?;
        let used: i64 = sqlx::query_scalar("SELECT count(*) FROM rec_assignments WHERE subject_id=$1::uuid AND created_at>now()-interval '1 day'")
            .bind(&subject).fetch_one(&mut *tx).await?;
        if used+feed.items.len() as i64>1000 { return Err(FeedbackError::Limited); }
        let mut kept = Vec::with_capacity(feed.items.len());
        for (index, mut item) in feed.items.into_iter().enumerate() {
            let book = book_id(&item.book_id)?;
            let ticket = new_ticket()?;
            let digest = token_digest(&ticket)?;
            let inserted: Option<String> = sqlx::query_scalar(
                "INSERT INTO rec_assignments(subject_id,session_digest,ticket_digest,book_id,surface,source,reason,position,generation_id)
                 SELECT $1::uuid,$2,$3,$4,$5,$6,$7,$8,$9::uuid
                 WHERE EXISTS(SELECT 1 FROM rec_public_books WHERE id=$4)
                   AND NOT EXISTS(SELECT 1 FROM rec_feedback_state WHERE subject_id=$1::uuid AND book_id=$4 AND hidden AND expires_at>now())
                 RETURNING id::text")
                .bind(&subject).bind(session.as_slice()).bind(digest.as_slice()).bind(book)
                .bind(surface.as_str()).bind(&item.source).bind(&item.reason).bind((index+1) as i16)
                .bind(feed.generation_id.as_deref()).fetch_optional(&mut *tx).await?;
            if inserted.is_some() { item.ticket=Some(ticket); kept.push(item); }
        }
        feed.items=kept;
        tx.commit().await?;
        Ok(feed)
    }

    pub async fn activate(&self, actor: i64, session: &[u8;32], ticket: &str) -> Result<i64, FeedbackError> {
        validate_actor(actor)?;
        let digest=token_digest(ticket)?;
        let mut tx=self.pool.begin().await?;
        let subject=lock_subject(&mut tx,actor).await?;
        let row=sqlx::query(
            "UPDATE rec_assignments a SET clicked_at=COALESCE(a.clicked_at,now())
             WHERE a.ticket_digest=$1 AND a.subject_id=$2::uuid AND a.session_digest=$3
               AND a.expires_at>now()
               AND EXISTS(SELECT 1 FROM rec_public_books b WHERE b.id=a.book_id)
               AND NOT EXISTS(SELECT 1 FROM rec_feedback_state f WHERE f.subject_id=a.subject_id AND f.book_id=a.book_id AND f.hidden AND f.expires_at>now())
             RETURNING a.id::text,a.book_id")
            .bind(digest.as_slice()).bind(&subject).bind(session.as_slice()).fetch_optional(&mut *tx).await?.ok_or(FeedbackError::NotFound)?;
        let assignment: String=row.try_get("id")?;
        let book: i64=row.try_get("book_id")?;
        sqlx::query("INSERT INTO rec_attribution_events(assignment_id,kind,occurred_at,trust) SELECT id,'click',clicked_at,'server_navigation' FROM rec_assignments WHERE id=$1::uuid ON CONFLICT(assignment_id,kind) DO NOTHING")
            .bind(&assignment).execute(&mut *tx).await?;
        dirty(&mut tx,&assignment).await?;
        tx.commit().await?;
        Ok(book)
    }

    /// Called ONLY after the public book handler resolved the actual book (200).
    pub async fn record_book_open(&self, actor: i64, session: &[u8;32], book: i64) -> Result<(), FeedbackError> {
        validate_actor(actor)?;
        validate_actor(book)?;
        let mut tx=self.pool.begin().await?;
        let subject=lock_subject(&mut tx,actor).await?;
        let id: Option<String>=sqlx::query_scalar(
            "SELECT id::text FROM rec_assignments WHERE subject_id=$1::uuid AND session_digest=$2 AND book_id=$3
               AND clicked_at>now()-interval '15 minutes'
               AND EXISTS(SELECT 1 FROM rec_public_books WHERE id=$3)
             ORDER BY clicked_at DESC,id ASC LIMIT 1 FOR UPDATE")
            .bind(&subject).bind(session.as_slice()).bind(book).fetch_optional(&mut *tx).await?;
        if let Some(id)=id {
            sqlx::query("UPDATE rec_assignments SET book_opened_at=COALESCE(book_opened_at,now()) WHERE id=$1::uuid")
                .bind(&id).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO rec_attribution_events(assignment_id,kind,occurred_at,trust) SELECT id,'book_opened',book_opened_at,'server_navigation' FROM rec_assignments WHERE id=$1::uuid ON CONFLICT(assignment_id,kind) DO NOTHING")
                .bind(&id).execute(&mut *tx).await?;
            dirty(&mut tx,&id).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
