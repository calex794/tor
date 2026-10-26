//! PostgreSQL durable cover jobs with SKIP LOCKED, version fencing and no S3 I/O.
use crate::domain::cover::{CoverClaim, CoverError, CoverVariant};
use crate::services::cover_jobs::CoverJobs;
use async_trait::async_trait;
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgCoverJobs {
    pool: PgPool,
}

impl PgCoverJobs {
    pub fn new(pool: PgPool) -> Self { Self { pool } }

    /// INTERNAL ONLY. Caller must have verified S3 HEAD+bounded GET and digest.
    /// It is intentionally not exposed as an HTTP handler.
    pub async fn register_verified_source(
        &self,
        book_id: i64,
        actor_id: i64,
        storage_key: &str,
        mime: &str,
        size: i64,
        digest: &[u8; 32],
    ) -> Result<i64, CoverError> {
        if !storage_key.starts_with("staging/covers/")
            || !matches!(mime, "image/png" | "image/jpeg" | "image/webp")
            || !(1..=8_388_608).contains(&size)
        {
            return Err(CoverError::Invalid);
        }
        sqlx::query_scalar::<_, i64>(
            "INSERT INTO cover_upload_objects
             (book_id,uploader_user_id,storage_key,content_type,size_bytes,content_sha256)
             SELECT b.id,$2,$3,$4,$5,$6 FROM books b
             WHERE b.id=$1 AND b.owner_user_id=$2
               AND NOT b.admin_hold AND NOT b.is_deleted
             RETURNING id",
        )
        .bind(book_id).bind(actor_id).bind(storage_key).bind(mime)
        .bind(size).bind(digest.as_slice())
        .fetch_optional(&self.pool).await.map_err(|_| CoverError::Unavailable)?
        .ok_or(CoverError::Forbidden)
    }

    pub async fn job_status(&self, job_id: i64) -> Result<Option<String>, CoverError> {
        sqlx::query_scalar("SELECT status FROM cover_variant_jobs WHERE id=$1")
            .bind(job_id)
            .fetch_optional(&self.pool).await.map_err(|_| CoverError::Unavailable)
    }

    pub async fn enqueue(
        &self, source_id: i64, book_id: i64, actor_id: i64,
        expected_revision: i64, recipe: &str,
    ) -> Result<i64, CoverError> {
        if source_id <= 0 || expected_revision < 0 || !(8..=100).contains(&recipe.len()) {
            return Err(CoverError::Invalid);
        }
        let inserted = sqlx::query_scalar::<_, i64>(
            "INSERT INTO cover_variant_jobs
               (source_object_id,book_id,actor_user_id,recipe_version,expected_slot_revision)
             SELECT s.id,s.book_id,s.uploader_user_id,$5,$4
               FROM cover_upload_objects s JOIN books b ON b.id=s.book_id
               LEFT JOIN book_cover_slots p ON p.book_id=s.book_id
              WHERE s.id=$1 AND s.book_id=$2 AND s.uploader_user_id=$3
                AND s.status='verified' AND b.owner_user_id=$3
                AND NOT b.admin_hold AND NOT b.is_deleted
                AND COALESCE(p.revision,0)=$4
             ON CONFLICT (source_object_id,recipe_version) DO NOTHING
             RETURNING id",
        )
        .bind(source_id).bind(book_id).bind(actor_id).bind(expected_revision).bind(recipe)
        .fetch_optional(&self.pool).await.map_err(|_| CoverError::Unavailable)?;
        if let Some(id) = inserted { return Ok(id); }
        // Idempotent replay of an identical source/recipe is allowed even
        // after the slot revision changes. Cross-actor/source replay is not.
        sqlx::query_scalar::<_, i64>(
            "SELECT j.id FROM cover_variant_jobs j
             WHERE j.source_object_id=$1 AND j.book_id=$2
               AND j.actor_user_id=$3 AND j.expected_slot_revision=$4
               AND j.recipe_version=$5",
        )
        .bind(source_id).bind(book_id).bind(actor_id).bind(expected_revision).bind(recipe)
        .fetch_optional(&self.pool).await.map_err(|_| CoverError::Unavailable)?
        .ok_or(CoverError::Conflict)
    }
}

fn digest(bytes: Vec<u8>) -> Result<[u8; 32], CoverError> {
    bytes.try_into().map_err(|_| CoverError::Unavailable)
}

#[async_trait]
impl CoverJobs for PgCoverJobs {
    async fn claim_next(&self) -> Result<Option<CoverClaim>, CoverError> {
        // One claim per caller even across workers and process restarts.
        let row = sqlx::query(
            "WITH selected AS (
               SELECT id FROM cover_variant_jobs
                WHERE attempts<8 AND (
                   (status='pending' AND run_after<=now())
                   OR (status IN ('running','ready') AND locked_until<now())
                )
                ORDER BY run_after,id FOR UPDATE SKIP LOCKED LIMIT 1
             )
             UPDATE cover_variant_jobs j SET
               status='running', lease_epoch=j.lease_epoch+1,
               attempts=j.attempts+1, locked_until=now()+INTERVAL '120 seconds',
               updated_at=now()
             FROM selected WHERE j.id=selected.id
             RETURNING j.id,j.book_id,j.actor_user_id,j.source_object_id,
                       j.lease_epoch,j.recipe_version",
        ).fetch_optional(&self.pool).await.map_err(|_| CoverError::Unavailable)?;
        let Some(row) = row else { return Ok(None); };
        let source_id: i64 = row.try_get("source_object_id").map_err(|_|CoverError::Unavailable)?;
        let source = sqlx::query(
            "SELECT storage_key,content_type,size_bytes,content_sha256
             FROM cover_upload_objects WHERE id=$1 AND status='verified'",
        )
        .bind(source_id).fetch_optional(&self.pool).await
        .map_err(|_| CoverError::Unavailable)?
        .ok_or(CoverError::Unavailable)?;
        Ok(Some(CoverClaim {
            job_id: row.try_get("id").map_err(|_|CoverError::Unavailable)?,
            book_id: row.try_get("book_id").map_err(|_|CoverError::Unavailable)?,
            actor_user_id: row.try_get("actor_user_id").map_err(|_|CoverError::Unavailable)?,
            source_key: source.try_get("storage_key").map_err(|_|CoverError::Unavailable)?,
            source_mime: source.try_get("content_type").map_err(|_|CoverError::Unavailable)?,
            source_size: source.try_get("size_bytes").map_err(|_|CoverError::Unavailable)?,
            source_sha256: digest(source.try_get("content_sha256").map_err(|_|CoverError::Unavailable)?)?,
            epoch: row.try_get("lease_epoch").map_err(|_|CoverError::Unavailable)?,
            recipe_version: row.try_get("recipe_version").map_err(|_|CoverError::Unavailable)?,
        }))
    }

    async fn existing_variants(&self, job_id: i64) -> Result<Vec<CoverVariant>, CoverError> {
        let rows = sqlx::query(
            "SELECT variant_name,storage_key,content_sha256,size_bytes
             FROM cover_variants WHERE job_id=$1 ORDER BY variant_name",
        ).bind(job_id).fetch_all(&self.pool).await.map_err(|_| CoverError::Unavailable)?;
        rows.into_iter().map(|row| {
            Ok(CoverVariant {
                name: row.try_get("variant_name").map_err(|_|CoverError::Unavailable)?,
                storage_key: row.try_get("storage_key").map_err(|_|CoverError::Unavailable)?,
                sha256: digest(row.try_get("content_sha256").map_err(|_|CoverError::Unavailable)?)?,
                size_bytes: row.try_get("size_bytes").map_err(|_|CoverError::Unavailable)?,
            })
        }).collect()
    }

    async fn record_pair(
        &self, job_id: i64, epoch: i64, variants: &[CoverVariant;2],
    ) -> Result<(), CoverError> {
        if variants[0].name != "main" || variants[1].name != "card"
            || variants.iter().any(|v|v.size_bytes<=0 || v.storage_key.len()>512)
        { return Err(CoverError::Invalid); }
        let mut tx=self.pool.begin().await.map_err(|_|CoverError::Unavailable)?;
        let lock=sqlx::query_scalar::<_,i64>(
            "SELECT id FROM cover_variant_jobs
             WHERE id=$1 AND lease_epoch=$2 AND status='running'
               AND locked_until>now() FOR UPDATE",
        ).bind(job_id).bind(epoch).fetch_optional(&mut *tx).await
        .map_err(|_|CoverError::Unavailable)?;
        if lock.is_none() {return Err(CoverError::LostLease);}
        for v in variants {
            sqlx::query(
                "INSERT INTO cover_variants(job_id,variant_name,storage_key,content_sha256,size_bytes)
                 VALUES($1,$2,$3,$4,$5) ON CONFLICT(job_id,variant_name) DO NOTHING",
            ).bind(job_id).bind(&v.name).bind(&v.storage_key)
             .bind(v.sha256.as_slice()).bind(v.size_bytes)
             .execute(&mut *tx).await.map_err(|_|CoverError::Unavailable)?;
        }
        // A retry may have immutable variants already. Never silently attach
        // an incompatible previous staging object.
        let rows=sqlx::query(
            "SELECT variant_name,storage_key,content_sha256,size_bytes
             FROM cover_variants WHERE job_id=$1 ORDER BY variant_name",
        ).bind(job_id).fetch_all(&mut *tx).await.map_err(|_|CoverError::Unavailable)?;
        if rows.len()!=2 {return Err(CoverError::Conflict);}
        for (row, expected) in rows.iter().zip(variants.iter().rev()) {
            let got_hash:Vec<u8> = row.try_get("content_sha256").map_err(|_|CoverError::Unavailable)?;
            if row.try_get::<String,_>("variant_name").map_err(|_|CoverError::Unavailable)? != expected.name
                || row.try_get::<String,_>("storage_key").map_err(|_|CoverError::Unavailable)? != expected.storage_key
                || got_hash.as_slice()!=expected.sha256
                || row.try_get::<i64,_>("size_bytes").map_err(|_|CoverError::Unavailable)? != expected.size_bytes
            {return Err(CoverError::Conflict);}
        }
        sqlx::query(
            "UPDATE cover_variant_jobs SET status='ready',updated_at=now()
             WHERE id=$1 AND lease_epoch=$2 AND locked_until>now()",
        ).bind(job_id).bind(epoch).execute(&mut *tx).await.map_err(|_|CoverError::Unavailable)?;
        tx.commit().await.map_err(|_|CoverError::Unavailable)?;
        Ok(())
    }

    async fn conclude(&self, job_id:i64, epoch:i64, applied:bool)->Result<(),CoverError>{
        let state=if applied {"done"} else {"superseded"};
        let r=sqlx::query(
            "UPDATE cover_variant_jobs SET status=$3,locked_until=NULL,updated_at=now()
             WHERE id=$1 AND lease_epoch=$2 AND status IN ('ready','running')
               AND locked_until>now()",
        ).bind(job_id).bind(epoch).bind(state)
         .execute(&self.pool).await.map_err(|_|CoverError::Unavailable)?;
        if r.rows_affected()!=1 {return Err(CoverError::LostLease);}
        Ok(())
    }

    async fn record_failure(
        &self, job_id:i64, epoch:i64, retryable:bool, code:&'static str,
    )->Result<(),CoverError>{
        if code.len()>100 || code.is_empty(){return Err(CoverError::Invalid);}
        let r=sqlx::query(
            "UPDATE cover_variant_jobs SET
               status=CASE WHEN $3 AND attempts<6 THEN 'pending' ELSE 'dead' END,
               run_after=now()+make_interval(secs=>LEAST(300,(1 << LEAST(attempts,8)))),
               locked_until=NULL,updated_at=now(),last_error_code=$4
             WHERE id=$1 AND lease_epoch=$2 AND status IN ('running','ready')
               AND locked_until>now()",
        ).bind(job_id).bind(epoch).bind(retryable).bind(code)
         .execute(&self.pool).await.map_err(|_|CoverError::Unavailable)?;
        if r.rows_affected()!=1 {return Err(CoverError::LostLease);}
        Ok(())
    }
}
