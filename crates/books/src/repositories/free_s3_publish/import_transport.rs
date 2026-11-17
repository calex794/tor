//! Durable SINGLE/PACK selection before any queued-import storage side effect.
//! A rollout flag chooses new jobs; it never changes an already selected job.
use super::{write_error, PgFreeS3Publication};
use crate::services::{
    authoring::AuthorError,
    chapter_file_import::{fingerprint, ImportedChapter},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FreeImportTransport {
    Single,
    Pack,
}

pub struct ImportTransportRequest<'a> {
    pub job: (i64, i64),
    pub actor: i64,
    pub book: i64,
    pub command_key: &'a str,
    pub chapters: &'a [ImportedChapter],
    pub prefer_pack: bool,
}

impl PgFreeS3Publication {
    pub async fn probe_import_transport(&self) -> Result<(), AuthorError> {
        sqlx::query("SELECT job_id FROM free_chapter_import_transports LIMIT 0")
            .execute(&self.pool)
            .await
            .map_err(write_error)?;
        Ok(())
    }

    pub async fn select_import_transport(
        &self,
        req: ImportTransportRequest<'_>,
    ) -> Result<FreeImportTransport, AuthorError> {
        let ImportTransportRequest {
            job: (job_id, epoch),
            actor,
            book,
            command_key,
            chapters,
            prefer_pack,
        } = req;
        super::super::postgres::chapter_batch_import::validate_batch(
            actor,
            book,
            command_key,
            chapters,
        )?;
        if job_id <= 0 || epoch <= 0 {
            return Err(AuthorError::Invalid);
        }
        let digest = fingerprint(book, chapters);
        let mut tx = self.pool.begin().await.map_err(write_error)?;
        let current: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM chapter_import_jobs WHERE id=$1 AND lease_epoch=$2
             AND actor_user_id=$3 AND book_id=$4 AND command_key=$5
             AND approved_fingerprint=$6 AND expected_chapter_count=$7
             AND status='running' AND locked_until>clock_timestamp() FOR UPDATE",
        )
        .bind(job_id)
        .bind(epoch)
        .bind(actor)
        .bind(book)
        .bind(command_key)
        .bind(digest.as_slice())
        .bind(chapters.len() as i32)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?;
        if current.is_none() {
            return Err(AuthorError::Conflict);
        }
        let saved: Option<String> = sqlx::query_scalar(
            "SELECT transport FROM free_chapter_import_transports WHERE job_id=$1",
        )
        .bind(job_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?;
        let mode = if let Some(mode) = saved {
            mode
        } else {
            // Adoption of older checkpoints is deterministic. Legacy SINGLE
            // reservations keep SINGLE; an existing exact PACK stage keeps PACK.
            let staged: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM free_pack_direct_import_stages WHERE job_id=$1)",
            )
            .bind(job_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(write_error)?;
            let reserved:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM free_chapter_import_reservations WHERE user_id=$1 AND command_key=$2)")
                .bind(actor).bind(command_key).fetch_one(&mut *tx).await.map_err(write_error)?;
            let mode = if staged || (prefer_pack && !reserved) {
                "pack"
            } else {
                "single"
            };
            sqlx::query("INSERT INTO free_chapter_import_transports
                (job_id,actor_user_id,book_id,command_key,approved_fingerprint,chapter_count,transport)
                VALUES($1,$2,$3,$4,$5,$6,$7)")
                .bind(job_id).bind(actor).bind(book).bind(command_key).bind(digest.as_slice())
                .bind(chapters.len() as i32).bind(mode).execute(&mut *tx).await.map_err(write_error)?;
            mode.to_owned()
        };
        tx.commit().await.map_err(write_error)?;
        match mode.as_str() {
            "single" => Ok(FreeImportTransport::Single),
            "pack" => Ok(FreeImportTransport::Pack),
            _ => Err(AuthorError::Conflict),
        }
    }
}
