//! DROP-R4c: bounded, read-only B pack planning inventory.
//! Only FREE immutable S349 receipts are eligible; no HTTP user may select
//! an arbitrary object or bypass owner/hold checks.
use super::free_pack_attach::PgFreePackCatalog;
use super::free_pack_sources::ApprovedPackSource;
use crate::services::authoring::AuthorError;
use sqlx::Row;

pub const MAX_PLANNER_MEMBERS: usize = 64;
pub const MAX_PLANNER_DECODED_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct PackPlannerPolicy {
    pub minimum_members: usize,
    pub maximum_members: usize,
    pub maximum_decoded_bytes: u64,
}

impl Default for PackPlannerPolicy {
    fn default() -> Self {
        Self {
            minimum_members: 16,
            maximum_members: MAX_PLANNER_MEMBERS,
            maximum_decoded_bytes: MAX_PLANNER_DECODED_BYTES,
        }
    }
}

impl PackPlannerPolicy {
    pub fn validate(self) -> Result<Self, AuthorError> {
        if !(2..=MAX_PLANNER_MEMBERS).contains(&self.minimum_members)
            || self.maximum_members < self.minimum_members
            || self.maximum_members > MAX_PLANNER_MEMBERS
            || !(1_000_000..=MAX_PLANNER_DECODED_BYTES)
                .contains(&self.maximum_decoded_bytes)
        {
            return Err(AuthorError::Invalid);
        }
        Ok(self)
    }
}

#[derive(Clone, Debug)]
pub struct PackPlannerBatch {
    pub book_id: i64,
    pub actor_user_id: i64,
    pub sources: Vec<ApprovedPackSource>,
}

impl PgFreePackCatalog {
    /// Paged by primary key: no full catalog scan, OFFSET walk or unbounded
    /// grouping over the projected 100k book / 60m chapter population.
    pub async fn planner_next_books(
        &self,
        after_book_id: i64,
        page_size: i64,
    ) -> Result<Vec<i64>, AuthorError> {
        if after_book_id < 0 || !(1..=32).contains(&page_size) {
            return Err(AuthorError::Invalid);
        }
        sqlx::query_scalar(
            "SELECT b.id FROM books b
             JOIN users u ON u.id=b.owner_user_id
             WHERE b.id>$1 AND b.visibility IN ('public','draft')
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND u.account_state='active'
               AND NOT EXISTS (
                 SELECT 1 FROM free_pack_jobs j
                 WHERE j.book_id=b.id AND j.status IN ('pending','running')
               )
             ORDER BY b.id LIMIT $2",
        )
        .bind(after_book_id)
        .bind(page_size)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)
    }

    /// List one book's prefix of eligible historical FREE editions.
    /// Even saved/older V1 editions are candidates, but ONLY as inactive PACK
    /// copies. A prior membership (including failed/cancelled) is excluded
    /// until an operator consciously resolves it; no automatic infinite loop.
    pub async fn planner_book_batch(
        &self,
        book_id: i64,
        private_bucket: &str,
        policy: PackPlannerPolicy,
    ) -> Result<Option<PackPlannerBatch>, AuthorError> {
        let policy = policy.validate()?;
        if book_id <= 0 || private_bucket.is_empty() || private_bucket.starts_with("public") {
            return Err(AuthorError::Invalid);
        }
        let owner: Option<i64> = sqlx::query_scalar(
            "SELECT b.owner_user_id FROM books b
             JOIN users u ON u.id=b.owner_user_id
             WHERE b.id=$1 AND u.account_state='active'
               AND b.visibility IN ('public','draft')
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND NOT EXISTS(
                 SELECT 1 FROM free_pack_jobs j
                 WHERE j.book_id=b.id AND j.status IN ('pending','running')
               )",
        )
        .bind(book_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let Some(actor_user_id) = owner else {
            return Ok(None);
        };
        let rows = sqlx::query(
            "SELECT c.id AS chapter_id,v.id AS version_id,
                    s.sha256,s.size_bytes,s.bucket
             FROM chapters c
             JOIN chapter_versions v ON v.chapter_id=c.id
             JOIN free_chapter_storage_objects s
               ON s.chapter_id=c.id AND s.version_id=v.id
             WHERE c.book_id=$1 AND c.access_level='free' AND c.price_amount=0
               AND NOT c.is_deleted AND NOT c.admin_hold
               AND v.content_sha256=s.sha256 AND s.bucket=$2
               AND NOT EXISTS(
                 SELECT 1 FROM free_edition_storage_generations g
                 WHERE g.chapter_id=c.id AND g.version_id=v.id
                   AND g.placement_kind='pack'
               )
               AND NOT EXISTS(
                 SELECT 1 FROM free_pack_job_members m
                 WHERE m.chapter_id=c.id AND m.version_id=v.id
               )
             ORDER BY c.id,v.id LIMIT $3",
        )
        .bind(book_id)
        .bind(private_bucket)
        .bind(policy.maximum_members as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let mut total = 0_u64;
        let mut sources = Vec::new();
        for row in rows {
            let size_i64: i64 = row.try_get("size_bytes").map_err(|_| AuthorError::Unavailable)?;
            if !(1..=200_000).contains(&size_i64) {
                return Err(AuthorError::Conflict);
            }
            let size_bytes = size_i64 as u64;
            let next = total.checked_add(size_bytes).ok_or(AuthorError::Invalid)?;
            if next > policy.maximum_decoded_bytes {
                break;
            }
            let digest: Vec<u8> = row.try_get("sha256").map_err(|_| AuthorError::Unavailable)?;
            let digest: [u8; 32] = digest.try_into().map_err(|_| AuthorError::Conflict)?;
            sources.push(ApprovedPackSource {
                chapter_id: row.try_get("chapter_id").map_err(|_| AuthorError::Unavailable)?,
                version_id: row.try_get("version_id").map_err(|_| AuthorError::Unavailable)?,
                content_sha256: digest,
                size_bytes,
                bucket: row.try_get("bucket").map_err(|_| AuthorError::Unavailable)?,
            });
            total = next;
        }
        if sources.len() < policy.minimum_members {
            return Ok(None);
        }
        Ok(Some(PackPlannerBatch { book_id, actor_user_id, sources }))
    }

    pub async fn probe_pack_planner_schema(&self) -> Result<(), AuthorError> {
        sqlx::query("SELECT job_id,chapter_id,version_id FROM free_pack_job_members WHERE false")
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        Ok(())
    }
}
