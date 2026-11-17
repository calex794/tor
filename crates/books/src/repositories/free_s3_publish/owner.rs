//! Exact owner-only working/published edition; no reader authorization here.
use super::{write_error, PgFreeS3Publication};
use crate::services::authoring::{AuthorError, ChapterAccess, OwnedChapter};
use sqlx::Row;

pub struct OwnedFreeS3Chapter {
    pub chapter: OwnedChapter,
    pub version_id: i64,
    pub published: bool,
    pub bucket: String,
    pub sha256: [u8; 32],
    pub size_bytes: u64,
    /// Only the current, verified PACK placement has no separate SINGLE.
    pub stored_in_pack: bool,
}

impl PgFreeS3Publication {
    pub async fn owned(
        &self,
        actor: i64,
        book: i64,
        chapter: i64,
    ) -> Result<Option<OwnedFreeS3Chapter>, AuthorError> {
        let row = sqlx::query("SELECT c.chapter_no,c.title,c.edit_revision,
            v.id AS version_id,p.version_id IS NOT NULL AS published,
            COALESCE(f.bucket,pack.bucket) AS bucket,
            COALESCE(f.sha256,g.content_sha256) AS sha256,
            COALESCE(f.size_bytes,g.size_bytes) AS size_bytes,
            f.chapter_id IS NULL AS stored_in_pack
            FROM books b JOIN chapters c ON c.book_id=b.id
            LEFT JOIN chapter_publications p ON p.chapter_id=c.id
            LEFT JOIN chapter_working_drafts d ON d.chapter_id=c.id
            JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=COALESCE(d.version_id,p.version_id)
            LEFT JOIN free_chapter_storage_objects f
              ON f.chapter_id=c.id AND f.version_id=v.id AND f.sha256=v.content_sha256
            LEFT JOIN free_edition_active_storage a
              ON a.chapter_id=v.chapter_id AND a.version_id=v.id
            LEFT JOIN free_edition_storage_generations g
              ON g.chapter_id=a.chapter_id AND g.version_id=a.version_id
             AND g.generation=a.generation AND g.placement_kind='pack'
             AND g.content_sha256=v.content_sha256
            LEFT JOIN free_storage_packs pack ON pack.id=g.pack_id
            WHERE b.id=$1 AND b.owner_user_id=$2 AND c.id=$3
              AND b.visibility IN ('public','draft') AND NOT b.admin_hold AND NOT b.is_deleted
              AND c.access_level='free' AND c.price_amount=0
              AND NOT c.admin_hold AND NOT c.is_deleted
              AND (f.chapter_id IS NOT NULL OR pack.id IS NOT NULL)")
            .bind(book).bind(actor).bind(chapter).fetch_optional(&self.pool)
            .await.map_err(write_error)?;
        row.map(|r| {
            let digest: Vec<u8> = r.try_get("sha256").map_err(write_error)?;
            let bytes: i64 = r.try_get("size_bytes").map_err(write_error)?;
            Ok(OwnedFreeS3Chapter {
                chapter: OwnedChapter {
                    number: r.try_get("chapter_no").map_err(write_error)?,
                    title: r.try_get("title").map_err(write_error)?,
                    revision: r.try_get("edit_revision").map_err(write_error)?,
                    access: ChapterAccess::Free,
                    price_amount: 0,
                    body: String::new(),
                },
                version_id: r.try_get("version_id").map_err(write_error)?,
                published: r.try_get("published").map_err(write_error)?,
                bucket: r.try_get("bucket").map_err(write_error)?,
                sha256: digest.try_into().map_err(|_| AuthorError::Unavailable)?,
                size_bytes: u64::try_from(bytes).map_err(|_| AuthorError::Unavailable)?,
                stored_in_pack: r.try_get("stored_in_pack").map_err(write_error)?,
            })
        })
        .transpose()
    }
}
