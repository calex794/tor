//! FND-00: B-owned metadata for the currently published paid S3-only edition.
//! Exact plaintext lives in G/private S3, not in this repository or DB result.
//! This is an author-only editor selector, NOT a reader entitlement.
use super::PgPaidStorage;
use crate::services::authoring::AuthorError;
use sqlx::Row;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct OwnedPaidBodylessMetadata {
    pub number: i32,
    pub title: String,
    pub price_amount: i64,
    pub revision: i64,
    pub version_id: i64,
}

impl PgPaidStorage {
    /// Return B metadata and the exact current edition identity only when
    /// the caller owns this published, verified S3-only paid chapter. The
    /// caller must fence the G source read to this version ID after network I/O.
    pub async fn owned_published_bodyless_paid_metadata(
        &self,
        actor: i64,
        book: i64,
        chapter: i64,
    ) -> Result<Option<OwnedPaidBodylessMetadata>, AuthorError> {
        if actor <= 0 || book <= 0 || chapter <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT c.chapter_no,c.title,c.price_amount,c.edit_revision,
                    v.id AS version_id
             FROM books b
             JOIN users owner ON owner.id=b.owner_user_id
                AND owner.account_state='active'
             JOIN chapters c ON c.book_id=b.id
             JOIN chapter_publications p ON p.chapter_id=c.id
             JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=p.version_id
             JOIN chapter_encrypted_release_events e
               ON e.chapter_id=c.id AND e.version_id=v.id
              AND e.storage_object_id=v.storage_object_id
             JOIN storage_objects o ON o.id=e.storage_object_id
             JOIN chapter_reader_manifests m
               ON m.chapter_id=c.id AND m.version_id=v.id
              AND m.storage_object_id=o.id
              AND m.content_sha256=v.content_sha256
             WHERE b.id=$1 AND b.owner_user_id=$2 AND c.id=$3
               AND b.visibility='public' AND b.visibility_status='active'
               AND c.visibility='public'
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND NOT c.admin_hold AND NOT c.is_deleted
               AND c.access_level='paid' AND c.price_amount>0
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
               AND v.body_markdown IS NULL
               AND v.content_sha256 IS NOT NULL
               AND v.content_sha256=o.plaintext_sha256
               AND o.state='attached' AND o.object_kind='chapter'
               AND o.codec='gzip' AND o.verified_at IS NOT NULL
               AND o.object_key='private/chapter-versions/' || o.id::text || '.enc'",
        )
        .bind(book)
        .bind(actor)
        .bind(chapter)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        row.map(|row| {
            Ok(OwnedPaidBodylessMetadata {
                number: row
                    .try_get("chapter_no")
                    .map_err(|_| AuthorError::Unavailable)?,
                title: row.try_get("title").map_err(|_| AuthorError::Unavailable)?,
                price_amount: row
                    .try_get("price_amount")
                    .map_err(|_| AuthorError::Unavailable)?,
                revision: row
                    .try_get("edit_revision")
                    .map_err(|_| AuthorError::Unavailable)?,
                version_id: row
                    .try_get("version_id")
                    .map_err(|_| AuthorError::Unavailable)?,
            })
        })
        .transpose()
    }
}
