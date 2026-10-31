//! S023: durable import queue. Only trusted code may register verified private S3 sources.
//! Chapters are NOT committed here: the final B transaction locks this job row.
use super::PgAuthoring;
use crate::services::authoring::{valid_command_key, AuthorError};
use sqlx::Row;

#[derive(Debug, Clone)]
pub struct ChapterImportClaim {
    pub job_id: i64,
    pub book_id: i64,
    pub actor_id: i64,
    pub command_key: String,
    pub source_key: String,
    pub source_filename: String,
    pub source_sha256: Vec<u8>,
    pub source_bytes: i64,
    pub approved_fingerprint: Vec<u8>,
    pub expected_chapter_count: i32,
    pub attempts: i32,
    pub epoch: i64,
}

#[derive(Debug, Clone)]
pub struct ChapterImportStatus {
    pub status: String,
    pub attempts: i32,
    pub saved_count: i32,
    pub error_code: Option<String>,
}

impl PgAuthoring {
    /// Check a prior submission before uploading. This also makes a replay safe
    /// after the worker has deleted the completed source object from S3.
    #[allow(clippy::too_many_arguments)]
    pub async fn existing_chapter_import(
        &self,
        actor: i64,
        book: i64,
        command_key: &str,
        filename: &str,
        hash: &[u8; 32],
        size: i64,
        fingerprint: &[u8; 32],
        count: i32,
    ) -> Result<Option<i64>, AuthorError> {
        let row = sqlx::query(
            "SELECT id,book_id,source_filename,source_sha256,source_bytes,
                    approved_fingerprint,expected_chapter_count
             FROM chapter_import_jobs WHERE actor_user_id=$1 AND command_key=$2",
        )
        .bind(actor)
        .bind(command_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let matches = row.try_get::<i64, _>("book_id").ok() == Some(book)
            && row.try_get::<String, _>("source_filename").ok().as_deref() == Some(filename)
            && row.try_get::<Vec<u8>, _>("source_sha256").ok().as_deref()
                == Some(hash.as_slice())
            && row.try_get::<i64, _>("source_bytes").ok() == Some(size)
            && row.try_get::<Vec<u8>, _>("approved_fingerprint").ok().as_deref()
                == Some(fingerprint.as_slice())
            && row.try_get::<i32, _>("expected_chapter_count").ok() == Some(count);
        if !matches {
            return Err(AuthorError::Conflict);
        }
        row.try_get("id")
            .map(Some)
            .map_err(|_| AuthorError::Unavailable)
    }

    /// INTERNAL: caller must independently verify S3 HEAD, bounded GET, hash and
    /// preview approval. Neither source_key nor hashes may come from arbitrary HTTP.
    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue_verified_chapter_import(
        &self,
        actor: i64,
        book: i64,
        command_key: &str,
        key: &str,
        filename: &str,
        hash: &[u8; 32],
        size: i64,
        fingerprint: &[u8; 32],
        count: i32,
    ) -> Result<i64, AuthorError> {
        let lower = filename.to_ascii_lowercase();
        if actor <= 0
            || book <= 0
            || !valid_command_key(command_key)
            || !key.starts_with("staging/chapters/")
            || key.contains("..")
            || key.len() < 32
            || key.len() > 512
            || !key.bytes().all(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'-' | b'_')
            })
            || filename.is_empty()
            || filename.len() > 128
            || filename.contains('/')
            || filename.contains('\\')
            || filename.chars().any(char::is_control)
            || !(lower.ends_with(".txt") || lower.ends_with(".md"))
            || !(1..=700_000).contains(&size)
            || !(1..=500).contains(&count)
        {
            return Err(AuthorError::Invalid);
        }
        let inserted: Option<i64> = sqlx::query_scalar(
            "INSERT INTO chapter_import_jobs
             (book_id,actor_user_id,command_key,source_key,source_filename,
              source_sha256,source_bytes,approved_fingerprint,expected_chapter_count)
             SELECT b.id,$2,$3,$4,$5,$6,$7,$8,$9
               FROM books b WHERE b.id=$1 AND b.owner_user_id=$2
               AND b.visibility='draft' AND NOT b.admin_hold AND NOT b.is_deleted
             ON CONFLICT (actor_user_id,command_key) DO NOTHING
             RETURNING id",
        )
        .bind(book)
        .bind(actor)
        .bind(command_key)
        .bind(key)
        .bind(filename)
        .bind(hash.as_slice())
        .bind(size)
        .bind(fingerprint.as_slice())
        .bind(count)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| match error {
            sqlx::Error::Database(ref db) if db.code().as_deref() == Some("23505") => {
                AuthorError::Conflict
            }
            _ => AuthorError::Unavailable,
        })?;
        if let Some(id) = inserted {
            return Ok(id);
        }
        self.existing_chapter_import(
            actor,
            book,
            command_key,
            filename,
            hash,
            size,
            fingerprint,
            count,
        )
        .await?
        .ok_or(AuthorError::Missing)
    }

    /// Leases are database-authoritative; expired leases can be reclaimed after a crash.
    pub async fn claim_chapter_import(&self) -> Result<Option<ChapterImportClaim>, AuthorError> {
        // Do not leave the eighth expired attempt permanently "running".
        sqlx::query(
            "UPDATE chapter_import_jobs SET status='failed',locked_until=NULL,
                    last_error_code='attempt_limit',updated_at=now(),finished_at=now()
             WHERE status='running' AND attempts>=8 AND locked_until<=now()",
        )
        .execute(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let row = sqlx::query(
            "WITH candidate AS (
               SELECT id FROM chapter_import_jobs
               WHERE attempts<8 AND (
                  (status='queued' AND run_after<=now()) OR
                  (status='running' AND locked_until<=now()))
               ORDER BY run_after,id FOR UPDATE SKIP LOCKED LIMIT 1
             )
             UPDATE chapter_import_jobs j SET status='running',
                 attempts=j.attempts+1,lease_epoch=j.lease_epoch+1,
                 locked_until=now()+INTERVAL '120 seconds',updated_at=now()
             FROM candidate WHERE j.id=candidate.id
             RETURNING j.id,j.book_id,j.actor_user_id,j.command_key,j.source_key,
               j.source_filename,j.source_sha256,j.source_bytes,
               j.approved_fingerprint,j.expected_chapter_count,j.attempts,j.lease_epoch",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        row.map(|row| {
            Ok(ChapterImportClaim {
                job_id: row.try_get("id").map_err(|_| AuthorError::Unavailable)?,
                book_id: row.try_get("book_id").map_err(|_| AuthorError::Unavailable)?,
                actor_id: row
                    .try_get("actor_user_id")
                    .map_err(|_| AuthorError::Unavailable)?,
                command_key: row
                    .try_get("command_key")
                    .map_err(|_| AuthorError::Unavailable)?,
                source_key: row
                    .try_get("source_key")
                    .map_err(|_| AuthorError::Unavailable)?,
                source_filename: row
                    .try_get("source_filename")
                    .map_err(|_| AuthorError::Unavailable)?,
                source_sha256: row
                    .try_get("source_sha256")
                    .map_err(|_| AuthorError::Unavailable)?,
                source_bytes: row
                    .try_get("source_bytes")
                    .map_err(|_| AuthorError::Unavailable)?,
                approved_fingerprint: row
                    .try_get("approved_fingerprint")
                    .map_err(|_| AuthorError::Unavailable)?,
                expected_chapter_count: row
                    .try_get("expected_chapter_count")
                    .map_err(|_| AuthorError::Unavailable)?,
                attempts: row.try_get("attempts").map_err(|_| AuthorError::Unavailable)?,
                epoch: row
                    .try_get("lease_epoch")
                    .map_err(|_| AuthorError::Unavailable)?,
            })
        })
        .transpose()
    }

    pub async fn owned_chapter_import_status(
        &self,
        actor: i64,
        book: i64,
        job: i64,
    ) -> Result<Option<ChapterImportStatus>, AuthorError> {
        let row = sqlx::query(
            "SELECT j.status,j.attempts,j.saved_count,j.last_error_code
             FROM chapter_import_jobs j JOIN books b ON b.id=j.book_id
             WHERE j.id=$1 AND j.book_id=$2 AND j.actor_user_id=$3
               AND b.owner_user_id=$3 AND NOT b.is_deleted AND NOT b.admin_hold",
        )
        .bind(job)
        .bind(book)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        row.map(|r| {
            Ok(ChapterImportStatus {
                status: r.try_get("status").map_err(|_| AuthorError::Unavailable)?,
                attempts: r.try_get("attempts").map_err(|_| AuthorError::Unavailable)?,
                saved_count: r.try_get("saved_count").map_err(|_| AuthorError::Unavailable)?,
                error_code: r
                    .try_get("last_error_code")
                    .map_err(|_| AuthorError::Unavailable)?,
            })
        })
        .transpose()
    }

    /// A row lock serializes cancellation with the final transaction's job lock.
    pub async fn cancel_chapter_import(
        &self,
        actor: i64,
        book: i64,
        job: i64,
    ) -> Result<bool, AuthorError> {
        let changed = sqlx::query(
            "UPDATE chapter_import_jobs j SET status='cancelled',locked_until=NULL,
                    lease_epoch=j.lease_epoch+1,updated_at=now(),finished_at=now()
             FROM books b WHERE j.id=$1 AND j.book_id=$2 AND j.actor_user_id=$3
               AND b.id=j.book_id AND b.owner_user_id=$3
               AND NOT b.is_deleted AND NOT b.admin_hold
               AND j.status IN ('queued','running')",
        )
        .bind(job)
        .bind(book)
        .bind(actor)
        .execute(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        Ok(changed.rows_affected() == 1)
    }

    pub async fn fail_chapter_import(
        &self,
        job: i64,
        epoch: i64,
        retry: bool,
        code: &str,
    ) -> Result<bool, AuthorError> {
        if code.is_empty()
            || code.len() > 100
            || !code
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b == b'_')
        {
            return Err(AuthorError::Invalid);
        }
        let status = if retry { "queued" } else { "failed" };
        let result = sqlx::query(
            "UPDATE chapter_import_jobs SET status=$3,locked_until=NULL,
               run_after=CASE WHEN $3='queued' THEN now()+INTERVAL '30 seconds' ELSE run_after END,
               last_error_code=$4,updated_at=now(),
               finished_at=CASE WHEN $3='failed' THEN now() ELSE NULL END
             WHERE id=$1 AND lease_epoch=$2 AND status='running' AND locked_until>now()",
        )
        .bind(job)
        .bind(epoch)
        .bind(status)
        .bind(code)
        .execute(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        Ok(result.rows_affected() == 1)
    }
}
