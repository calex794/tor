//! B-owned, durable public-cover lifecycle ledger for S025.
//! Do not enable a CDN publisher until it reserves an identity before S3 COPY,
//! and a separate worker processes removals plus Yandex CDN purge.
use sqlx::{PgPool, Row};

use super::cover_slots::PublicCoverKind;

#[derive(Clone)]
pub struct PgPublicCoverPublications {
    pool: PgPool,
}

#[derive(Clone, Eq, PartialEq)]
pub struct PublicCoverPublication {
    pub id: i64,
    pub book_id: i64,
    pub job_id: i64,
    pub slot_revision: i64,
    pub variant_name: String,
    pub sha256: [u8; 32],
    pub size_bytes: i64,
    pub lease_epoch: i64,
}

impl PublicCoverPublication {
    pub fn object_key(&self) -> String {
        let digest: String = self.sha256.iter().map(|b| format!("{b:02x}")).collect();
        format!(
            "public/covers/{}/{}/{}-{digest}.webp",
            self.book_id, self.slot_revision, self.variant_name
        )
    }

    /// A suffix wildcard also invalidates segmented and Vary-dependent CDN copies.
    pub fn purge_path(&self) -> String {
        format!("/{}*", self.object_key())
    }
}

fn valid_cdn_operation_id(operation_id: &str) -> bool {
    !operation_id.is_empty()
        && operation_id.len() <= 128
        && operation_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl PgPublicCoverPublications {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn kind_name(kind: PublicCoverKind) -> &'static str {
        match kind {
            PublicCoverKind::Card => "card",
            PublicCoverKind::Main => "main",
        }
    }

    fn from_row(row: &sqlx::postgres::PgRow) -> Result<PublicCoverPublication, sqlx::Error> {
        let digest: Vec<u8> = row.try_get("content_sha256")?;
        let sha256: [u8; 32] = digest
            .try_into()
            .map_err(|_| sqlx::Error::Protocol("invalid public cover SHA-256".into()))?;
        Ok(PublicCoverPublication {
            id: row.try_get("id")?,
            book_id: row.try_get("book_id")?,
            job_id: row.try_get("job_id")?,
            slot_revision: row.try_get("slot_revision")?,
            variant_name: row.try_get("variant_name")?,
            sha256,
            size_bytes: row.try_get("size_bytes")?,
            lease_epoch: row.try_get("lease_epoch")?,
        })
    }

    /// Lock the B-owned Book and cover pointers, then record the exact identity
    /// in the *same transaction*. The caller must commit this BEFORE S3 COPY.
    /// Conflicting contents at the same Book/revision/variant fail closed.
    pub async fn reserve_current(
        &self,
        book_id: i64,
        kind: PublicCoverKind,
    ) -> Result<Option<PublicCoverPublication>, sqlx::Error> {
        if book_id <= 0 {
            return Ok(None);
        }
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT s.job_id, s.revision, v.content_sha256, v.size_bytes
             FROM books b
             JOIN book_cover_slots s ON s.book_id=b.id
             JOIN cover_variant_jobs j ON j.id=s.job_id AND j.book_id=b.id
             JOIN cover_variants v ON v.job_id=j.id AND v.variant_name=$2
             WHERE b.id=$1
               AND b.visibility='public' AND b.visibility_status='active'
               AND b.lifecycle_status<>'pending_deletion' AND b.access_level='basic'
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND b.owner_user_id=j.actor_user_id AND j.status='done'
             FOR SHARE OF b,s,j,v",
        )
        .bind(book_id)
        .bind(Self::kind_name(kind))
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let job_id: i64 = row.try_get("job_id")?;
        let slot_revision: i64 = row.try_get("revision")?;
        let digest: Vec<u8> = row.try_get("content_sha256")?;
        let sha256: [u8; 32] = digest
            .try_into()
            .map_err(|_| sqlx::Error::Protocol("invalid public cover SHA-256".into()))?;
        let size_bytes: i64 = row.try_get("size_bytes")?;
        if !(1..=8 * 1024 * 1024).contains(&size_bytes) || slot_revision <= 0 {
            return Err(sqlx::Error::Protocol(
                "invalid public cover size/revision".into(),
            ));
        }
        let reserved = sqlx::query(
            "INSERT INTO public_cover_publications
                (book_id,job_id,slot_revision,variant_name,content_sha256,size_bytes)
             VALUES ($1,$2,$3,$4,$5,$6)
             ON CONFLICT (book_id,slot_revision,variant_name) DO UPDATE
                 SET updated_at=now()
             WHERE public_cover_publications.job_id=EXCLUDED.job_id
               AND public_cover_publications.content_sha256=EXCLUDED.content_sha256
               AND public_cover_publications.size_bytes=EXCLUDED.size_bytes
               AND public_cover_publications.state IN ('reserved','copied')
             RETURNING id,book_id,job_id,slot_revision,variant_name,
                       content_sha256,size_bytes,lease_epoch",
        )
        .bind(book_id)
        .bind(job_id)
        .bind(slot_revision)
        .bind(Self::kind_name(kind))
        .bind(&sha256[..])
        .bind(size_bytes)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        reserved.as_ref().map(Self::from_row).transpose()
    }

    /// Call only after a successful S3 copy. A hidden/replaced Book cannot
    /// transition to 'copied'; its identity becomes durable removal work.
    pub async fn confirm_copy(&self, id: i64) -> Result<bool, sqlx::Error> {
        if id <= 0 {
            return Ok(false);
        }
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT id,book_id,job_id,slot_revision,variant_name,
                    content_sha256,size_bytes,lease_epoch,state
             FROM public_cover_publications WHERE id=$1 FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            return Ok(false);
        };
        let state: String = row.try_get("state")?;
        if state != "reserved" && state != "copied" {
            return Ok(false);
        }
        let publication = Self::from_row(&row)?;
        let current = sqlx::query(
            "SELECT 1 FROM books b
             JOIN book_cover_slots s ON s.book_id=b.id
             JOIN cover_variant_jobs j ON j.id=s.job_id AND j.book_id=b.id
             JOIN cover_variants v ON v.job_id=j.id AND v.variant_name=$5
             WHERE b.id=$1 AND s.job_id=$2 AND s.revision=$3
               AND v.content_sha256=$4 AND v.size_bytes=$6
               AND b.visibility='public' AND b.visibility_status='active'
               AND b.lifecycle_status<>'pending_deletion' AND b.access_level='basic'
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND b.owner_user_id=j.actor_user_id AND j.status='done'
             FOR SHARE OF b,s,j,v",
        )
        .bind(publication.book_id)
        .bind(publication.job_id)
        .bind(publication.slot_revision)
        .bind(&publication.sha256[..])
        .bind(&publication.variant_name)
        .bind(publication.size_bytes)
        .fetch_optional(&mut *tx)
        .await?;
        let valid = current.is_some();
        sqlx::query(
            "UPDATE public_cover_publications
             SET state=$2,updated_at=now() WHERE id=$1",
        )
        .bind(id)
        .bind(if valid { "copied" } else { "removal_due" })
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(valid)
    }

    /// Return only fully confirmed copies of the currently attached, PUBLIC
    /// BASIC cover variants. A reservation is never a publishable CDN address.
    /// Recheck Book visibility here rather than trusting an earlier HTML query.
    pub async fn current_cdn_copies(
        &self,
        book_ids: &[i64],
        kind: PublicCoverKind,
    ) -> Result<Vec<PublicCoverPublication>, sqlx::Error> {
        if book_ids.is_empty() {
            return Ok(Vec::new());
        }
        if book_ids.len() > 100 || book_ids.iter().any(|id| *id <= 0) {
            return Err(sqlx::Error::Protocol(
                "invalid public CDN cover lookup batch".into(),
            ));
        }
        let rows = sqlx::query(
            "SELECT p.id,p.book_id,p.job_id,p.slot_revision,p.variant_name,
                    p.content_sha256,p.size_bytes,p.lease_epoch
             FROM public_cover_publications p
             JOIN books b ON b.id=p.book_id
             JOIN book_cover_slots s ON s.book_id=b.id
               AND s.job_id=p.job_id AND s.revision=p.slot_revision
             JOIN cover_variant_jobs j ON j.id=p.job_id AND j.book_id=b.id
             JOIN cover_variants v ON v.job_id=j.id
               AND v.variant_name=p.variant_name
               AND v.content_sha256=p.content_sha256
               AND v.size_bytes=p.size_bytes
             WHERE p.book_id=ANY($1) AND p.variant_name=$2
               AND p.state='copied'
               AND b.visibility='public' AND b.visibility_status='active'
               AND b.lifecycle_status<>'pending_deletion' AND b.access_level='basic'
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND b.owner_user_id=j.actor_user_id AND j.status='done'
             ORDER BY p.book_id",
        )
        .bind(book_ids)
        .bind(Self::kind_name(kind))
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(Self::from_row).collect()
    }

    /// Recover stale copies after Book hide/replacement, including copies for
    /// which the app crashed before finalizing. A reserved copy is only eligible
    /// after its bounded publication window; the publisher must respect it.
    /// This method claims exact immutable identities, never arbitrary S3 keys.
    pub async fn claim_removals(
        &self,
        limit: i64,
    ) -> Result<Vec<PublicCoverPublication>, sqlx::Error> {
        if !(1..=50).contains(&limit) {
            return Err(sqlx::Error::Protocol(
                "invalid public cover cleanup limit".into(),
            ));
        }
        let rows = sqlx::query(
            "WITH due AS (
                SELECT p.id FROM public_cover_publications p
                WHERE (p.lease_until IS NULL OR p.lease_until < now())
                  AND (p.retry_after IS NULL OR p.retry_after <= now())
                  AND (
                    p.state='removal_due'
                    OR ((p.state='reserved' AND p.created_at < now() - interval '15 minutes'
                         OR p.state='copied') AND NOT EXISTS (
                        SELECT 1 FROM books b
                        JOIN book_cover_slots s ON s.book_id=b.id
                        JOIN cover_variant_jobs j ON j.id=s.job_id AND j.book_id=b.id
                        JOIN cover_variants v ON v.job_id=j.id AND v.variant_name=p.variant_name
                        WHERE b.id=p.book_id AND s.job_id=p.job_id
                          AND s.revision=p.slot_revision
                          AND v.content_sha256=p.content_sha256
                          AND v.size_bytes=p.size_bytes
                          AND b.visibility='public' AND b.visibility_status='active'
                          AND b.lifecycle_status<>'pending_deletion' AND b.access_level='basic'
                          AND NOT b.admin_hold AND NOT b.is_deleted
                          AND b.owner_user_id=j.actor_user_id AND j.status='done'
                    ))
                  )
                ORDER BY p.id
                LIMIT $1 FOR UPDATE OF p SKIP LOCKED
            )
            UPDATE public_cover_publications p SET
                state='removal_due',lease_epoch=p.lease_epoch+1,
                lease_until=now()+interval '1 minute',updated_at=now()
            FROM due WHERE p.id=due.id
            RETURNING p.id,p.book_id,p.job_id,p.slot_revision,p.variant_name,
                      p.content_sha256,p.size_bytes,p.lease_epoch",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(Self::from_row).collect()
    }

    /// Fence stale workers: only the holder of an unexpired lease may confirm
    /// deletion from the public S3 origin. CDN purge still has to follow.
    pub async fn origin_deleted(&self, id: i64, epoch: i64) -> Result<bool, sqlx::Error> {
        let changed = sqlx::query(
            "UPDATE public_cover_publications
             SET state='origin_deleted',lease_until=NULL,retry_after=NULL,updated_at=now()
             WHERE id=$1 AND lease_epoch=$2
               AND state='removal_due' AND lease_until>now()",
        )
        .bind(id)
        .bind(epoch)
        .execute(&self.pool)
        .await?;
        Ok(changed.rows_affected() == 1)
    }

    pub async fn retry_removal(&self, id: i64, epoch: i64) -> Result<bool, sqlx::Error> {
        let changed = sqlx::query(
            "UPDATE public_cover_publications
             SET lease_until=NULL,retry_after=now()+interval '1 minute',
                 attempts=attempts+1,updated_at=now()
             WHERE id=$1 AND lease_epoch=$2
               AND state='removal_due' AND lease_until>now()",
        )
        .bind(id)
        .bind(epoch)
        .execute(&self.pool)
        .await?;
        Ok(changed.rows_affected() == 1)
    }

    /// A submitted purge is not yet a completed CDN invalidation.
    pub async fn purge_submitted(&self, id: i64, operation_id: &str) -> Result<bool, sqlx::Error> {
        if operation_id.is_empty() || operation_id.len() > 128 {
            return Ok(false);
        }
        let changed = sqlx::query(
            "UPDATE public_cover_publications
             SET state='purge_submitted',cdn_operation_id=$2,updated_at=now()
             WHERE id=$1 AND state='origin_deleted'",
        )
        .bind(id)
        .bind(operation_id)
        .execute(&self.pool)
        .await?;
        Ok(changed.rows_affected() == 1)
    }

    /// Use only after Yandex Operation.Get reports done=true with no error.
    pub async fn purge_completed(&self, id: i64, operation_id: &str) -> Result<bool, sqlx::Error> {
        let changed = sqlx::query(
            "UPDATE public_cover_publications
             SET state='purged',updated_at=now()
             WHERE id=$1 AND state='purge_submitted' AND cdn_operation_id=$2",
        )
        .bind(id)
        .bind(operation_id)
        .execute(&self.pool)
        .await?;
        Ok(changed.rows_affected() == 1)
    }
    /// Acquire at most three origin-deleted identities for selective CDN purge.
    /// A short lease fences concurrent workers, including after a restart.
    pub async fn claim_purge_submissions(
        &self,
        limit: i64,
    ) -> Result<Vec<PublicCoverPublication>, sqlx::Error> {
        if !(1..=3).contains(&limit) {
            return Err(sqlx::Error::Protocol("invalid CDN purge batch".into()));
        }
        let rows = sqlx::query(
            "WITH due AS (
                SELECT id FROM public_cover_publications
                WHERE state='origin_deleted'
                  AND (lease_until IS NULL OR lease_until<now())
                  AND (retry_after IS NULL OR retry_after<=now())
                ORDER BY id LIMIT $1 FOR UPDATE SKIP LOCKED
             )
             UPDATE public_cover_publications p
             SET lease_epoch=p.lease_epoch+1,
                 lease_until=now()+interval '1 minute',
                 updated_at=now()
             FROM due WHERE p.id=due.id
             RETURNING p.id,p.book_id,p.job_id,p.slot_revision,
                       p.variant_name,p.content_sha256,p.size_bytes,p.lease_epoch",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(Self::from_row).collect()
    }

    pub async fn purge_submitted_fenced(
        &self,
        id: i64,
        epoch: i64,
        operation_id: &str,
    ) -> Result<bool, sqlx::Error> {
        if !valid_cdn_operation_id(operation_id) {
            return Ok(false);
        }
        let changed = sqlx::query(
            "UPDATE public_cover_publications
             SET state='purge_submitted',cdn_operation_id=$3,
                 lease_until=NULL,retry_after=now()+interval '15 seconds',
                 updated_at=now()
             WHERE id=$1 AND lease_epoch=$2 AND state='origin_deleted'
               AND lease_until>now()",
        )
        .bind(id)
        .bind(epoch)
        .bind(operation_id)
        .execute(&self.pool)
        .await?;
        Ok(changed.rows_affected() == 1)
    }

    /// An unknown POST outcome may have succeeded at the provider. The next
    /// worker can safely submit the same exact immutable path a second time.
    pub async fn retry_purge_submission(&self, id: i64, epoch: i64) -> Result<bool, sqlx::Error> {
        let changed = sqlx::query(
            "UPDATE public_cover_publications
             SET lease_until=NULL,retry_after=now()+interval '1 minute',
                 attempts=attempts+1,updated_at=now()
             WHERE id=$1 AND lease_epoch=$2 AND state='origin_deleted'
               AND lease_until>now()",
        )
        .bind(id)
        .bind(epoch)
        .execute(&self.pool)
        .await?;
        Ok(changed.rows_affected() == 1)
    }

    pub async fn claim_purge_polls(
        &self,
        limit: i64,
    ) -> Result<Vec<(PublicCoverPublication, String)>, sqlx::Error> {
        if !(1..=3).contains(&limit) {
            return Err(sqlx::Error::Protocol("invalid CDN poll batch".into()));
        }
        let rows = sqlx::query(
            "WITH due AS (
                SELECT id FROM public_cover_publications
                WHERE state='purge_submitted' AND cdn_operation_id IS NOT NULL
                  AND (lease_until IS NULL OR lease_until<now())
                  AND (retry_after IS NULL OR retry_after<=now())
                ORDER BY id LIMIT $1 FOR UPDATE SKIP LOCKED
             )
             UPDATE public_cover_publications p
             SET lease_epoch=p.lease_epoch+1,
                 lease_until=now()+interval '1 minute',
                 updated_at=now()
             FROM due WHERE p.id=due.id
             RETURNING p.id,p.book_id,p.job_id,p.slot_revision,
                       p.variant_name,p.content_sha256,p.size_bytes,
                       p.lease_epoch,p.cdn_operation_id",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| Ok((Self::from_row(row)?, row.try_get("cdn_operation_id")?)))
            .collect()
    }

    pub async fn purge_completed_fenced(
        &self,
        id: i64,
        epoch: i64,
        operation_id: &str,
    ) -> Result<bool, sqlx::Error> {
        let changed = sqlx::query(
            "UPDATE public_cover_publications
             SET state='purged',lease_until=NULL,retry_after=NULL,
                 updated_at=now()
             WHERE id=$1 AND lease_epoch=$2 AND cdn_operation_id=$3
               AND state='purge_submitted' AND lease_until>now()",
        )
        .bind(id)
        .bind(epoch)
        .bind(operation_id)
        .execute(&self.pool)
        .await?;
        Ok(changed.rows_affected() == 1)
    }

    /// Provider-reported failure: clear the operation and return to origin_deleted.
    pub async fn purge_failed_fenced(
        &self,
        id: i64,
        epoch: i64,
        operation_id: &str,
    ) -> Result<bool, sqlx::Error> {
        let changed = sqlx::query(
            "UPDATE public_cover_publications
             SET state='origin_deleted',cdn_operation_id=NULL,
                 lease_until=NULL,retry_after=now()+interval '1 minute',
                 attempts=attempts+1,updated_at=now()
             WHERE id=$1 AND lease_epoch=$2 AND cdn_operation_id=$3
               AND state='purge_submitted' AND lease_until>now()",
        )
        .bind(id)
        .bind(epoch)
        .bind(operation_id)
        .execute(&self.pool)
        .await?;
        Ok(changed.rows_affected() == 1)
    }

    /// Pending, timed out or malformed status: retain the operation identity.
    pub async fn defer_purge_poll(
        &self,
        id: i64,
        epoch: i64,
        operation_id: &str,
    ) -> Result<bool, sqlx::Error> {
        let changed = sqlx::query(
            "UPDATE public_cover_publications
             SET lease_until=NULL,retry_after=now()+interval '30 seconds',
                 attempts=attempts+1,updated_at=now()
             WHERE id=$1 AND lease_epoch=$2 AND cdn_operation_id=$3
               AND state='purge_submitted' AND lease_until>now()",
        )
        .bind(id)
        .bind(epoch)
        .bind(operation_id)
        .execute(&self.pool)
        .await?;
        Ok(changed.rows_affected() == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::PublicCoverPublication;

    #[test]
    fn exact_immutable_key_and_segment_safe_yandex_purge_path() {
        let value = PublicCoverPublication {
            id: 1,
            book_id: 950,
            job_id: 20,
            slot_revision: 7,
            variant_name: "card".into(),
            sha256: [0xab; 32],
            size_bytes: 100,
            lease_epoch: 0,
        };
        let expected = format!("public/covers/950/7/card-{}.webp", "ab".repeat(32));
        assert_eq!(value.object_key(), expected);
        assert_eq!(value.purge_path(), format!("/{expected}*"));
        assert!(!value.purge_path().contains(".."));
    }
}
