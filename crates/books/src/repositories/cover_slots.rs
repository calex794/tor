//! B-owned cover-slot transition. G never updates book_cover_slots.
//! A single PG commit verifies actor, staged main/card pair and revision CAS.
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgCoverSlots {
    pool: PgPool,
}
impl PgCoverSlots {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn current_slot(&self, book_id: i64) -> Result<Option<(i64, i64)>, sqlx::Error> {
        let row = sqlx::query("SELECT job_id,revision FROM book_cover_slots WHERE book_id=$1")
            .bind(book_id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| Ok((row.try_get("job_id")?, row.try_get("revision")?)))
            .transpose()
    }

    /// Returns false if the actor changed, a later cover won, or B has a hold.
    /// Same job/current slot is an idempotent success after a worker crash.
    pub async fn attach_ready_job(
        &self,
        job_id: i64,
        lease_epoch: i64,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let job = sqlx::query(
            "SELECT j.book_id,j.expected_slot_revision FROM cover_variant_jobs j
             JOIN books b ON b.id=j.book_id
             WHERE j.id=$1 AND j.lease_epoch=$2
               AND j.status IN ('running','ready') AND j.locked_until>now()
               AND b.owner_user_id=j.actor_user_id
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND (SELECT count(*) FROM cover_variants v WHERE v.job_id=j.id)=2
               AND EXISTS(SELECT 1 FROM cover_variants v WHERE v.job_id=j.id AND v.variant_name='main')
               AND EXISTS(SELECT 1 FROM cover_variants v WHERE v.job_id=j.id AND v.variant_name='card')
             FOR UPDATE OF b,j",
        ).bind(job_id).bind(lease_epoch).fetch_optional(&mut *tx).await?;
        let Some(job) = job else {
            return Ok(false);
        };
        let book_id: i64 = job.try_get("book_id")?;
        let expected: i64 = job.try_get("expected_slot_revision")?;
        let slot =
            sqlx::query("SELECT job_id,revision FROM book_cover_slots WHERE book_id=$1 FOR UPDATE")
                .bind(book_id)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(slot) = slot {
            if slot.try_get::<i64, _>("job_id")? == job_id {
                tx.commit().await?;
                return Ok(true);
            }
            if slot.try_get::<i64, _>("revision")? != expected {
                return Ok(false);
            }
        } else if expected != 0 {
            return Ok(false);
        }
        let changed = sqlx::query(
            "INSERT INTO book_cover_slots(book_id,job_id,revision)
             VALUES($1,$2,1)
             ON CONFLICT(book_id) DO UPDATE
               SET job_id=EXCLUDED.job_id,revision=book_cover_slots.revision+1,
                   attached_at=now()
               WHERE book_cover_slots.revision=$3
             RETURNING revision",
        )
        .bind(book_id)
        .bind(job_id)
        .bind(expected)
        .fetch_optional(&mut *tx)
        .await?;
        if changed.is_none() {
            return Ok(false);
        }
        tx.commit().await?;
        Ok(true)
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum PublicCoverKind {
    Card,
    Main,
}

impl PublicCoverKind {
    pub fn from_route(value: &str) -> Option<Self> {
        match value {
            "card" => Some(Self::Card),
            "main" => Some(Self::Main),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Card => "card",
            Self::Main => "main",
        }
    }
}

/// Server-only internal read model: never render the private S3 key into HTML,
/// JSON or a redirect Location header. The HTTP layer must proxy and verify bytes.
#[derive(Clone, Eq, PartialEq)]
pub struct PublicCoverObject {
    pub storage_key: String,
    pub sha256: [u8; 32],
    pub size_bytes: i64,
    pub revision: i64,
}

impl PgCoverSlots {
    /// Reads only the currently attached, completed cover of a visible book.
    /// All visibility and current-owner checks happen in the SQL query.
    /// No cached result may bypass a subsequent unpublish/hold/delete operation.
    pub async fn public_variant(
        &self,
        book_id: i64,
        kind: PublicCoverKind,
    ) -> Result<Option<PublicCoverObject>, sqlx::Error> {
        if book_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT v.storage_key,v.content_sha256,v.size_bytes,s.revision
             FROM books b
             JOIN book_cover_slots s ON s.book_id=b.id
             JOIN cover_variant_jobs j ON j.id=s.job_id AND j.book_id=b.id
             JOIN cover_variants v ON v.job_id=j.id AND v.variant_name=$2
             WHERE b.id=$1 AND b.visibility='public'
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND b.owner_user_id=j.actor_user_id
               AND j.status='done'",
        )
        .bind(book_id)
        .bind(kind.as_str())
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let storage_key: String = row.try_get("storage_key")?;
        let digest: Vec<u8> = row.try_get("content_sha256")?;
        let size_bytes: i64 = row.try_get("size_bytes")?;
        let revision: i64 = row.try_get("revision")?;
        let sha256: [u8; 32] = digest
            .try_into()
            .map_err(|_| sqlx::Error::Protocol("cover SHA-256 length mismatch".into()))?;
        if size_bytes <= 0
            || size_bytes > 8 * 1024 * 1024
            || revision <= 0
            || !storage_key.starts_with("staging/covers/variants/")
            || storage_key.len() > 512
            || storage_key.contains("..")
            || storage_key.contains("//")
            || !storage_key.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'-' | b'_')
            })
        {
            return Err(sqlx::Error::Protocol(
                "invalid stored public cover variant metadata".into(),
            ));
        }
        Ok(Some(PublicCoverObject {
            storage_key,
            sha256,
            size_bytes,
            revision,
        }))
    }
}

impl PgCoverSlots {
    /// A bounded single-query projection for H/F cards, with exactly the same
    /// fail-closed visibility, ownership and attached-variant predicates as
    /// public_variant. A client-supplied id list can never reveal an S3 key.
    pub async fn public_card_ids(
        &self,
        ids: &[u64],
    ) -> Result<std::collections::HashSet<u64>, sqlx::Error> {
        if ids.is_empty() {
            return Ok(std::collections::HashSet::new());
        }
        if ids.len() > 100 {
            return Err(sqlx::Error::Protocol(
                "cover projection batch too large".into(),
            ));
        }
        let ids = ids
            .iter()
            .map(|id| {
                i64::try_from(*id).map_err(|_| sqlx::Error::Protocol("invalid book id".into()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let rows: Vec<i64> = sqlx::query_scalar(
            "SELECT b.id FROM books b
             JOIN book_cover_slots s ON s.book_id=b.id
             JOIN cover_variant_jobs j ON j.id=s.job_id AND j.book_id=b.id
             JOIN cover_variants v ON v.job_id=j.id AND v.variant_name='card'
             WHERE b.id=ANY($1::bigint[])
               AND b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
               AND b.owner_user_id=j.actor_user_id AND j.status='done'",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|id| {
                u64::try_from(id)
                    .map_err(|_| sqlx::Error::Protocol("invalid stored book id".into()))
            })
            .collect()
    }
}

#[cfg(test)]
mod public_cover_tests {
    use super::PublicCoverKind;

    #[test]
    fn accepts_only_bounded_cover_variants() {
        assert!(PublicCoverKind::from_route("card").is_some());
        assert!(PublicCoverKind::from_route("main").is_some());
        for invalid in ["", "CARD", "../card", "source", "thumb", "card/../main"] {
            assert!(PublicCoverKind::from_route(invalid).is_none());
        }
    }
}
