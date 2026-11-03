//! G-owned PostgreSQL queue. No chapter writes or S3 I/O occur in this repository.
use crate::domain::{ChapterImportClaim, ChapterImportStatus, ImportJobsError};
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgImportJobs {
    pool: PgPool,
}

impl PgImportJobs {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn probe(&self) -> Result<(), ImportJobsError> {
        sqlx::query("SELECT id FROM chapter_import_jobs LIMIT 0")
            .execute(&self.pool)
            .await
            .map_err(|_| ImportJobsError::Unavailable)?;
        Ok(())
    }

    /// Check an earlier command before S3 upload. A replay remains safe after
    /// a terminal worker has deleted its private staging object.
    #[allow(clippy::too_many_arguments)]
    pub async fn existing_for_command(
        &self,
        actor: i64,
        book: i64,
        command_key: &str,
        filename: &str,
        hash: &[u8; 32],
        size: i64,
        fingerprint: &[u8; 32],
        count: i32,
    ) -> Result<Option<i64>, ImportJobsError> {
        let row = sqlx::query(
            "SELECT id,book_id,source_filename,source_sha256,source_bytes,
                    approved_fingerprint,expected_chapter_count
             FROM chapter_import_jobs WHERE actor_user_id=$1 AND command_key=$2",
        )
        .bind(actor)
        .bind(command_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| ImportJobsError::Unavailable)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let matches = row.try_get::<i64, _>("book_id").ok() == Some(book)
            && row.try_get::<String, _>("source_filename").ok().as_deref() == Some(filename)
            && row.try_get::<Vec<u8>, _>("source_sha256").ok().as_deref() == Some(hash.as_slice())
            && row.try_get::<i64, _>("source_bytes").ok() == Some(size)
            && row
                .try_get::<Vec<u8>, _>("approved_fingerprint")
                .ok()
                .as_deref()
                == Some(fingerprint.as_slice())
            && row.try_get::<i32, _>("expected_chapter_count").ok() == Some(count);
        if !matches {
            return Err(ImportJobsError::Conflict);
        }
        row.try_get("id")
            .map(Some)
            .map_err(|_| ImportJobsError::Unavailable)
    }

    /// INTERNAL: the caller has already written and verified the private S3 object.
    /// This method validates ownership and persists only immutable object metadata.
    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue_verified(
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
    ) -> Result<i64, ImportJobsError> {
        let lower = filename.to_ascii_lowercase();
        if actor <= 0
            || book <= 0
            || !valid_command_key(command_key)
            || key != format!("staging/chapters/{actor}/{book}/{command_key}")
            || key.contains("..")
            || key.len() < 32
            || key.len() > 512
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'-' | b'_'))
            || filename.is_empty()
            || filename.len() > 128
            || filename.contains('/')
            || filename.contains('\\')
            || filename.chars().any(char::is_control)
            || !(lower.ends_with(".txt") || lower.ends_with(".md"))
            || !(1..=700_000).contains(&size)
            || !(1..=500).contains(&count)
        {
            return Err(ImportJobsError::Invalid);
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
                ImportJobsError::Conflict
            }
            _ => ImportJobsError::Unavailable,
        })?;
        if let Some(id) = inserted {
            return Ok(id);
        }
        self.existing_for_command(
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
        .ok_or(ImportJobsError::Missing)
    }

    /// Atomic SKIP LOCKED claim. An expired lease increments the fencing epoch.
    pub async fn claim_next(&self) -> Result<Option<ChapterImportClaim>, ImportJobsError> {
        sqlx::query(
            "UPDATE chapter_import_jobs SET status='failed',locked_until=NULL,
                    last_error_code='attempt_limit',updated_at=now(),finished_at=now()
             WHERE status='running' AND attempts>=8 AND locked_until<=now()",
        )
        .execute(&self.pool)
        .await
        .map_err(|_| ImportJobsError::Unavailable)?;
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
        .map_err(|_| ImportJobsError::Unavailable)?;
        row.map(|row| {
            Ok(ChapterImportClaim {
                job_id: row
                    .try_get("id")
                    .map_err(|_| ImportJobsError::Unavailable)?,
                book_id: row
                    .try_get("book_id")
                    .map_err(|_| ImportJobsError::Unavailable)?,
                actor_id: row
                    .try_get("actor_user_id")
                    .map_err(|_| ImportJobsError::Unavailable)?,
                command_key: row
                    .try_get("command_key")
                    .map_err(|_| ImportJobsError::Unavailable)?,
                source_key: row
                    .try_get("source_key")
                    .map_err(|_| ImportJobsError::Unavailable)?,
                source_filename: row
                    .try_get("source_filename")
                    .map_err(|_| ImportJobsError::Unavailable)?,
                source_sha256: row
                    .try_get("source_sha256")
                    .map_err(|_| ImportJobsError::Unavailable)?,
                source_bytes: row
                    .try_get("source_bytes")
                    .map_err(|_| ImportJobsError::Unavailable)?,
                approved_fingerprint: row
                    .try_get("approved_fingerprint")
                    .map_err(|_| ImportJobsError::Unavailable)?,
                expected_chapter_count: row
                    .try_get("expected_chapter_count")
                    .map_err(|_| ImportJobsError::Unavailable)?,
                attempts: row
                    .try_get("attempts")
                    .map_err(|_| ImportJobsError::Unavailable)?,
                epoch: row
                    .try_get("lease_epoch")
                    .map_err(|_| ImportJobsError::Unavailable)?,
            })
        })
        .transpose()
    }

    pub async fn latest_active_for_owner(
        &self,
        actor: i64,
        book: i64,
    ) -> Result<Option<(i64, ChapterImportStatus)>, ImportJobsError> {
        let row = sqlx::query(
            "SELECT j.id,j.status,j.attempts,j.saved_count,j.last_error_code
             FROM chapter_import_jobs j JOIN books b ON b.id=j.book_id
             WHERE j.book_id=$1 AND j.actor_user_id=$2
               AND b.owner_user_id=$2 AND NOT b.is_deleted AND NOT b.admin_hold
               AND j.status IN ('queued','running')
             ORDER BY j.id DESC LIMIT 1",
        )
        .bind(book)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| ImportJobsError::Unavailable)?;
        row.map(|r| {
            Ok((
                r.try_get("id").map_err(|_| ImportJobsError::Unavailable)?,
                decode_status(&r)?,
            ))
        })
        .transpose()
    }

    pub async fn status_for_owner(
        &self,
        actor: i64,
        book: i64,
        job: i64,
    ) -> Result<Option<ChapterImportStatus>, ImportJobsError> {
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
        .map_err(|_| ImportJobsError::Unavailable)?;
        row.map(|r| decode_status(&r)).transpose()
    }

    /// Return the internal key only to trusted cleanup code after cancellation.
    pub async fn cancel_source_for_owner(
        &self,
        actor: i64,
        book: i64,
        job: i64,
    ) -> Result<Option<String>, ImportJobsError> {
        sqlx::query_scalar(
            "UPDATE chapter_import_jobs j SET status='cancelled',locked_until=NULL,
                    lease_epoch=j.lease_epoch+1,updated_at=now(),finished_at=now()
             FROM books b WHERE j.id=$1 AND j.book_id=$2 AND j.actor_user_id=$3
               AND b.id=j.book_id AND b.owner_user_id=$3
               AND NOT b.is_deleted AND NOT b.admin_hold
               AND j.status IN ('queued','running')
             RETURNING j.source_key",
        )
        .bind(job)
        .bind(book)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| ImportJobsError::Unavailable)
    }

    pub async fn fail_if_current(
        &self,
        job: i64,
        epoch: i64,
        retry: bool,
        code: &str,
    ) -> Result<bool, ImportJobsError> {
        if code.is_empty()
            || code.len() > 100
            || !code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
        {
            return Err(ImportJobsError::Invalid);
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
        .map_err(|_| ImportJobsError::Unavailable)?;
        Ok(result.rows_affected() == 1)
    }
}

fn decode_status(row: &sqlx::postgres::PgRow) -> Result<ChapterImportStatus, ImportJobsError> {
    Ok(ChapterImportStatus {
        status: row
            .try_get("status")
            .map_err(|_| ImportJobsError::Unavailable)?,
        attempts: row
            .try_get("attempts")
            .map_err(|_| ImportJobsError::Unavailable)?,
        saved_count: row
            .try_get("saved_count")
            .map_err(|_| ImportJobsError::Unavailable)?,
        error_code: row
            .try_get("last_error_code")
            .map_err(|_| ImportJobsError::Unavailable)?,
    })
}

fn valid_command_key(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}
