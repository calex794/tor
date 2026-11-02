//! S058: durable first-release scheduling. No reader visibility until B-06 commit.
use super::PgAuthoring;
use crate::services::authoring::AuthorError;
use sqlx::Row;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledChapter {
    pub scheduled_utc: String,
    pub status: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DueReleaseOutcome { Published, Stale }

impl PgAuthoring {
    /// Owner-only, version-pinned schedule. The UTC field is YYYY-MM-DDTHH:MM.
    pub async fn schedule_pending_free_chapter(
        &self, actor: i64, book: i64, chapter: i64, revision: i64,
        version: i64, utc: &str,
    ) -> Result<(), AuthorError> {
        if actor <= 0 || book <= 0 || chapter <= 0 || revision <= 0 || version <= 0
            || utc.len() != 16 || !utc.bytes().enumerate().all(|(i,b)| {
                match i { 4 | 7 => b==b'-', 10 => b==b'T',
                          13 => b==b':', _ => b.is_ascii_digit() }
            })
        { return Err(AuthorError::Invalid); }
        let epoch: i64 = sqlx::query_scalar(
            "SELECT EXTRACT(EPOCH FROM ($1::timestamp AT TIME ZONE 'UTC'))::bigint")
            .bind(utc).fetch_one(&self.pool).await
            .map_err(|_| AuthorError::Invalid)?;
        let valid: bool = sqlx::query_scalar(
            "SELECT to_timestamp($1::double precision) > clock_timestamp() + interval '60 seconds'
              AND to_timestamp($1::double precision) <= clock_timestamp() + interval '365 days'")
            .bind(epoch).fetch_one(&self.pool).await
            .map_err(|_| AuthorError::Unavailable)?;
        if !valid { return Err(AuthorError::Invalid); }
        let mut tx = self.pool.begin().await.map_err(|_| AuthorError::Unavailable)?;
        let row = sqlx::query(
            "SELECT c.edit_revision,d.version_id FROM chapters c
             JOIN books b ON b.id=c.book_id
             JOIN chapter_working_drafts d ON d.chapter_id=c.id
             WHERE c.id=$1 AND c.book_id=$2 AND b.owner_user_id=$3
               AND b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
               AND c.visibility='draft' AND c.access_level='free' AND c.price_amount=0
               AND NOT c.admin_hold AND NOT c.is_deleted
             FOR UPDATE OF b,c")
            .bind(chapter).bind(book).bind(actor)
            .fetch_optional(&mut *tx).await.map_err(|_| AuthorError::Unavailable)?
            .ok_or(AuthorError::Missing)?;
        if row.try_get::<i64,_>("edit_revision").map_err(|_| AuthorError::Unavailable)? != revision
            || row.try_get::<i64,_>("version_id").map_err(|_| AuthorError::Unavailable)? != version
        { return Err(AuthorError::Conflict); }
        let saved: Option<i64> = sqlx::query_scalar(
            "INSERT INTO chapter_scheduled_releases
               (chapter_id,book_id,owner_user_id,version_id,expected_revision,scheduled_at)
             VALUES($1,$2,$3,$4,$5,to_timestamp($6::double precision))
             ON CONFLICT(chapter_id) DO UPDATE SET
               book_id=EXCLUDED.book_id,owner_user_id=EXCLUDED.owner_user_id,
               version_id=EXCLUDED.version_id,expected_revision=EXCLUDED.expected_revision,
               scheduled_at=EXCLUDED.scheduled_at,status='pending',lease_until=NULL,
               published_at=NULL,updated_at=now()
             WHERE chapter_scheduled_releases.status IN ('pending','cancelled','stale')
             RETURNING chapter_id")
            .bind(chapter).bind(book).bind(actor).bind(version).bind(revision).bind(epoch)
            .fetch_optional(&mut *tx).await.map_err(|_| AuthorError::Unavailable)?;
        if saved.is_none() { return Err(AuthorError::Conflict); }
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(())
    }

    /// Cancellation is refused after a worker claim; never report a false cancel.
    pub async fn cancel_scheduled_free_chapter(
        &self, actor: i64, book: i64, chapter: i64,
    ) -> Result<(), AuthorError> {
        if actor<=0 || book<=0 || chapter<=0 { return Err(AuthorError::Invalid); }
        let mut tx = self.pool.begin().await.map_err(|_| AuthorError::Unavailable)?;
        let status: Option<String> = sqlx::query_scalar(
            "SELECT s.status FROM chapter_scheduled_releases s
             JOIN books b ON b.id=s.book_id
             WHERE s.chapter_id=$1 AND s.book_id=$2 AND b.owner_user_id=$3
             FOR UPDATE OF s,b")
            .bind(chapter).bind(book).bind(actor).fetch_optional(&mut *tx).await
            .map_err(|_| AuthorError::Unavailable)?;
        match status.as_deref() {
            None => Err(AuthorError::Missing),
            Some("cancelled") => Ok(()),
            Some("pending") | Some("stale") => {
                sqlx::query(
                    "UPDATE chapter_scheduled_releases SET status='cancelled',
                     lease_until=NULL,updated_at=now() WHERE chapter_id=$1")
                    .bind(chapter).execute(&mut *tx).await
                    .map_err(|_| AuthorError::Unavailable)?;
                tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
                Ok(())
            }
            Some(_) => Err(AuthorError::Conflict),
        }
    }

    pub async fn scheduled_free_chapter(
        &self, actor: i64, book: i64, chapter: i64,
    ) -> Result<Option<ScheduledChapter>, AuthorError> {
        let row = sqlx::query(
            "SELECT to_char(s.scheduled_at AT TIME ZONE 'UTC',
                            'YYYY-MM-DD\"T\"HH24:MI') AS scheduled_utc,s.status
             FROM chapter_scheduled_releases s JOIN books b ON b.id=s.book_id
             WHERE s.chapter_id=$1 AND s.book_id=$2 AND b.owner_user_id=$3")
            .bind(chapter).bind(book).bind(actor).fetch_optional(&self.pool).await
            .map_err(|_| AuthorError::Unavailable)?;
        row.map(|row| Ok(ScheduledChapter {
            scheduled_utc: row.try_get("scheduled_utc").map_err(|_| AuthorError::Unavailable)?,
            status: row.try_get("status").map_err(|_| AuthorError::Unavailable)?,
        })).transpose()
    }

    /// Atomic lease + epoch fence. A crashed worker is retried after expiry;
    /// B-06 owns the publication transaction and unique first-release event.
    pub async fn process_one_due_scheduled_release(
        &self,
    ) -> Result<Option<(i64,DueReleaseOutcome)>, AuthorError> {
        let row = sqlx::query(
            "WITH picked AS (
               SELECT chapter_id FROM chapter_scheduled_releases
               WHERE (status='pending' AND scheduled_at<=clock_timestamp())
                  OR (status='claimed' AND lease_until<=clock_timestamp())
               ORDER BY scheduled_at,chapter_id FOR UPDATE SKIP LOCKED LIMIT 1
             )
             UPDATE chapter_scheduled_releases s
             SET status='claimed',claim_generation=s.claim_generation+1,
                 lease_until=clock_timestamp()+interval '5 minutes',updated_at=now()
             FROM picked WHERE s.chapter_id=picked.chapter_id
             RETURNING s.chapter_id,s.book_id,s.owner_user_id,
                       s.expected_revision,s.version_id,s.claim_generation")
            .fetch_optional(&self.pool).await.map_err(|_| AuthorError::Unavailable)?;
        let Some(row) = row else { return Ok(None); };
        let chapter: i64 = row.try_get("chapter_id").map_err(|_| AuthorError::Unavailable)?;
        let book: i64 = row.try_get("book_id").map_err(|_| AuthorError::Unavailable)?;
        let actor: i64 = row.try_get("owner_user_id").map_err(|_| AuthorError::Unavailable)?;
        let revision: i64 = row.try_get("expected_revision").map_err(|_| AuthorError::Unavailable)?;
        let version: i64 = row.try_get("version_id").map_err(|_| AuthorError::Unavailable)?;
        let generation: i64 = row.try_get("claim_generation").map_err(|_| AuthorError::Unavailable)?;
        let outcome = match self.release_pending_free_chapter(
            actor,book,chapter,revision,version).await {
            Ok(()) => DueReleaseOutcome::Published,
            Err(AuthorError::Unavailable) => return Err(AuthorError::Unavailable),
            Err(_) => DueReleaseOutcome::Stale,
        };
        let status = match outcome {
            DueReleaseOutcome::Published => "published",
            DueReleaseOutcome::Stale => "stale",
        };
        sqlx::query(
            "UPDATE chapter_scheduled_releases SET status=$3,lease_until=NULL,
             published_at=CASE WHEN $3='published' THEN now() ELSE NULL END,
             updated_at=now()
             WHERE chapter_id=$1 AND claim_generation=$2 AND status='claimed'")
            .bind(chapter).bind(generation).bind(status).execute(&self.pool).await
            .map_err(|_| AuthorError::Unavailable)?;
        Ok(Some((chapter,outcome)))
    }

    pub async fn probe_scheduled_releases(&self) -> Result<(), AuthorError> {
        sqlx::query("SELECT chapter_id FROM chapter_scheduled_releases LIMIT 0")
            .execute(&self.pool).await.map_err(|_| AuthorError::Unavailable)?;
        Ok(())
    }
}
