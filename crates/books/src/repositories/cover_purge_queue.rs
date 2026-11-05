//! S025: B-owned, leased CDN invalidation queue (never exposed to HTTP).
//! A successful S3 delete changes state to origin_deleted; only a verified
//! Yandex Operation.Get result may mark an immutable public cover purged.
use sqlx::{PgPool, Row};

use super::cover_publications::PublicCoverPublication;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PurgeStage {
    Submit,
    Poll,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CdnPurgeClaim {
    pub id: i64,
    pub book_id: i64,
    pub job_id: i64,
    pub slot_revision: i64,
    pub variant_name: String,
    pub sha256: [u8; 32],
    pub size_bytes: i64,
    pub epoch: i64,
    pub stage: PurgeStage,
    pub operation_id: Option<String>,
}

impl CdnPurgeClaim {
    pub fn purge_path(&self) -> String {
        PublicCoverPublication {
            id: self.id,
            book_id: self.book_id,
            job_id: self.job_id,
            slot_revision: self.slot_revision,
            variant_name: self.variant_name.clone(),
            sha256: self.sha256,
            size_bytes: self.size_bytes,
            lease_epoch: self.epoch,
        }
        .purge_path()
    }
}

#[derive(Clone)]
pub struct PgCdnPurgeQueue {
    pool: PgPool,
}

impl PgCdnPurgeQueue {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_claim(row: &sqlx::postgres::PgRow) -> Result<CdnPurgeClaim, sqlx::Error> {
        let digest: Vec<u8> = row.try_get("content_sha256")?;
        let sha256: [u8; 32] = digest
            .try_into()
            .map_err(|_| sqlx::Error::Protocol("invalid CDN cover hash".into()))?;
        let state: String = row.try_get("state")?;
        let operation_id: Option<String> = row.try_get("cdn_operation_id")?;
        let stage = match state.as_str() {
            "origin_deleted" if operation_id.is_none() => PurgeStage::Submit,
            "purge_submitted" if operation_id.is_some() => PurgeStage::Poll,
            _ => return Err(sqlx::Error::Protocol("invalid CDN purge state".into())),
        };
        Ok(CdnPurgeClaim {
            id: row.try_get("id")?,
            book_id: row.try_get("book_id")?,
            job_id: row.try_get("job_id")?,
            slot_revision: row.try_get("slot_revision")?,
            variant_name: row.try_get("variant_name")?,
            sha256,
            size_bytes: row.try_get("size_bytes")?,
            epoch: row.try_get("lease_epoch")?,
            stage,
            operation_id,
        })
    }

    /// At most two calls per one-minute lease; the provider has a 12-second
    /// request deadline. SKIP LOCKED prevents parallel workers claiming an
    /// immutable identity simultaneously, including after process restarts.
    pub async fn claim_batch(&self, limit: i64) -> Result<Vec<CdnPurgeClaim>, sqlx::Error> {
        if !(1..=2).contains(&limit) {
            return Err(sqlx::Error::Protocol("invalid CDN purge batch".into()));
        }
        let rows = sqlx::query(
            "WITH due AS (
                SELECT p.id FROM public_cover_publications p
                WHERE p.state IN ('origin_deleted', 'purge_submitted')
                  AND (p.lease_until IS NULL OR p.lease_until < now())
                  AND (p.retry_after IS NULL OR p.retry_after <= now())
                ORDER BY p.id LIMIT $1 FOR UPDATE OF p SKIP LOCKED
            )
            UPDATE public_cover_publications p SET
                lease_epoch=p.lease_epoch+1,
                lease_until=now()+interval '1 minute',updated_at=now()
            FROM due WHERE p.id=due.id
            RETURNING p.id,p.book_id,p.job_id,p.slot_revision,p.variant_name,
                      p.content_sha256,p.size_bytes,p.lease_epoch,
                      p.cdn_operation_id,p.state",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(Self::parse_claim).collect()
    }

    fn valid_operation_id(id: &str) -> bool {
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    }

    /// POST was accepted and returned an operation ID. That is not success:
    /// mark only "purge_submitted" and resume polling on a later claim.
    pub async fn mark_submitted(
        &self,
        id: i64,
        epoch: i64,
        operation_id: &str,
    ) -> Result<bool, sqlx::Error> {
        if !Self::valid_operation_id(operation_id) {
            return Ok(false);
        }
        let result = sqlx::query(
            "UPDATE public_cover_publications SET
                 state='purge_submitted',cdn_operation_id=$3,
                 lease_until=NULL,retry_after=now()+interval '30 seconds',
                 updated_at=now()
             WHERE id=$1 AND lease_epoch=$2
               AND state='origin_deleted' AND lease_until>now()",
        )
        .bind(id)
        .bind(epoch)
        .bind(operation_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Only Operation.Get done=true, response={}, error absent can call this.
    /// The same operation and current lease must still be present.
    pub async fn mark_completed(
        &self,
        id: i64,
        epoch: i64,
        operation_id: &str,
    ) -> Result<bool, sqlx::Error> {
        if !Self::valid_operation_id(operation_id) {
            return Ok(false);
        }
        let result = sqlx::query(
            "UPDATE public_cover_publications SET
                state='purged',lease_until=NULL,retry_after=NULL,updated_at=now()
             WHERE id=$1 AND lease_epoch=$2
               AND state='purge_submitted' AND cdn_operation_id=$3
               AND lease_until>now()",
        )
        .bind(id)
        .bind(epoch)
        .bind(operation_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Network failure: retain operation identity for polling (or retry POST
    /// of the exact same path when no response was received).
    pub async fn reschedule(&self, id: i64, epoch: i64, failed: bool) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE public_cover_publications SET
                lease_until=NULL,
                retry_after=now() +
                    CASE WHEN $3 THEN interval '1 minute' ELSE interval '30 seconds' END,
                purge_attempts=purge_attempts+
                    CASE WHEN $3 THEN 1 ELSE 0 END,
                updated_at=now()
             WHERE id=$1 AND lease_epoch=$2
               AND state IN ('origin_deleted','purge_submitted')
               AND lease_until>now()",
        )
        .bind(id)
        .bind(epoch)
        .bind(failed)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Operation.Get reported a terminal error: the next attempt must submit
    /// a fresh exact-path purge, never falsely acknowledge completion.
    pub async fn retry_failed_operation(
        &self,
        id: i64,
        epoch: i64,
        operation_id: &str,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE public_cover_publications SET
                state='origin_deleted',cdn_operation_id=NULL,lease_until=NULL,
                retry_after=now()+interval '1 minute',
                purge_attempts=purge_attempts+1,updated_at=now()
             WHERE id=$1 AND lease_epoch=$2 AND state='purge_submitted'
               AND cdn_operation_id=$3 AND lease_until>now()",
        )
        .bind(id)
        .bind(epoch)
        .bind(operation_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::{CdnPurgeClaim, PurgeStage};

    #[test]
    fn one_immutable_file_and_segments_only() {
        let claim = CdnPurgeClaim {
            id: 1,
            book_id: 950,
            job_id: 22,
            slot_revision: 3,
            variant_name: "card".to_owned(),
            sha256: [0xab; 32],
            size_bytes: 123,
            epoch: 1,
            stage: PurgeStage::Submit,
            operation_id: None,
        };
        assert_eq!(
            claim.purge_path(),
            format!("/public/covers/950/3/card-{}.webp*", "ab".repeat(32))
        );
    }
}
