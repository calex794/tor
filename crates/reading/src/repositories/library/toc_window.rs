//! C/H: private archive TOC keyset. Keep the existing saved Book page unchanged.
use super::*;
use crate::services::library::{SavedArchiveTocCursor, SavedArchiveTocSide};

impl PgLibrary {
    /// Bidirectional keyset for the private finalized archive's TOC.
    /// All rows repeat the existing active-session, publication/age,
    /// ordinary-deletion and canonical D purchase predicates.
    /// The caller must first verify a real pinned-free anchor from this book.
    /// A cursor is only ordering metadata, never a paid or free text grant.
    pub async fn saved_archive_toc_window(
        &self,
        user_id: i64,
        book_id: i64,
        limit: u16,
        cursor: SavedArchiveTocCursor,
        side: SavedArchiveTocSide,
        session_digest: &[u8; 32],
    ) -> Result<crate::services::library::SavedArchiveTocPage, LibraryError> {
        use crate::services::library::{
            SavedArchiveChapter, SavedArchiveTocPage, SavedPurchasedEdition,
        };
        if user_id <= 0
            || book_id <= 0
            || limit == 0
            || limit > 60
            || cursor.chapter_no <= 0
            || cursor.chapter_id <= 0
        {
            return Err(LibraryError::Invalid);
        }
        let (comparison, order) = match side {
            SavedArchiveTocSide::Before => ("<", "DESC"),
            SavedArchiveTocSide::After => (">", "ASC"),
            SavedArchiveTocSide::AtOrAfter => (">=", "ASC"),
        };
        // The two SQL fragments come exclusively from a Rust enum, not URL input.
        let statement = format!(
            "WITH eligible_book AS MATERIALIZED ( \
                 SELECT b.id FROM books b \
                 JOIN users viewer ON viewer.id=$1 AND viewer.account_state='active' \
                 JOIN sessions s ON s.user_id=viewer.id \
                   AND s.token_hash=$6::bytea AND s.revoked_at IS NULL \
                   AND s.expires_at>now() \
                 WHERE b.id=$2 AND b.visibility_status='active' \
                   AND (b.visibility='public' OR b.lifecycle_status='pending_deletion' \
                        OR (b.lifecycle_status='ordinary_deleted' AND b.ordinary_deleted_at IS NOT NULL)) \
                   AND NOT b.admin_hold AND NOT b.is_deleted \
                   AND (b.access_level='basic' \
                        OR (b.access_level='adult' AND viewer.adult_enabled) \
                        OR (b.access_level='pro_adult' AND EXISTS ( \
                            SELECT 1 FROM user_level_grants g \
                            WHERE g.user_id=viewer.id AND g.level='pro_adult' AND g.is_active \
                        ))) \
             ) \
             SELECT c.id AS chapter_id, c.chapter_no, c.title AS chapter_title, \
                    free_version.version_no AS free_version_no, \
                    paid.id AS purchase_id, paid_version.version_no AS paid_version_no \
             FROM eligible_book b \
             JOIN chapters c ON c.book_id=b.id \
               AND (c.chapter_no,c.id) {comparison} ($3::INTEGER,$4::BIGINT) \
             LEFT JOIN reader_saved_free_chapters f \
               ON f.user_id=$1 AND f.chapter_id=c.id \
              AND c.visibility='public' AND NOT c.admin_hold AND NOT c.is_deleted \
             LEFT JOIN chapter_versions free_version \
               ON free_version.chapter_id=f.chapter_id AND free_version.id=f.version_id \
             LEFT JOIN library_purchased_chapters l \
               ON l.user_id=$1 AND l.chapter_id=c.id \
             LEFT JOIN chapter_purchase_facts paid \
               ON paid.id=l.purchase_id AND paid.user_id=l.user_id \
              AND paid.chapter_id=l.chapter_id AND paid.version_id=l.active_version_id \
              AND tor_purchase_version_readable($1,paid.id,paid.chapter_id,paid.version_id) \
             LEFT JOIN chapter_versions paid_version \
               ON paid_version.chapter_id=paid.chapter_id AND paid_version.id=paid.version_id \
             WHERE free_version.id IS NOT NULL OR paid.id IS NOT NULL \
             ORDER BY c.chapter_no {order}, c.id {order} LIMIT $5",
        );
        let rows = sqlx::query(&statement)
            .bind(user_id)
            .bind(book_id)
            .bind(cursor.chapter_no)
            .bind(cursor.chapter_id)
            .bind(i64::from(limit) + 1)
            .bind(session_digest.as_slice())
            .fetch_all(&self.pool)
            .await
            .map_err(|_| LibraryError::Unavailable)?;
        let mut items = rows
            .into_iter()
            .map(|r| {
                let purchase_id: Option<i64> = r
                    .try_get("purchase_id")
                    .map_err(|_| LibraryError::Unavailable)?;
                let paid_version: Option<i32> = r
                    .try_get("paid_version_no")
                    .map_err(|_| LibraryError::Unavailable)?;
                Ok(SavedArchiveChapter {
                    chapter_id: r
                        .try_get("chapter_id")
                        .map_err(|_| LibraryError::Unavailable)?,
                    chapter_no: r
                        .try_get("chapter_no")
                        .map_err(|_| LibraryError::Unavailable)?,
                    chapter_title: r
                        .try_get("chapter_title")
                        .map_err(|_| LibraryError::Unavailable)?,
                    saved_free_version_no: r
                        .try_get("free_version_no")
                        .map_err(|_| LibraryError::Unavailable)?,
                    purchased: purchase_id
                        .zip(paid_version)
                        .map(|(purchase_id, version_no)| SavedPurchasedEdition {
                            purchase_id,
                            version_no,
                        }),
                })
            })
            .collect::<Result<Vec<_>, LibraryError>>()?;
        let has_more = items.len() > usize::from(limit);
        items.truncate(usize::from(limit));
        if matches!(side, SavedArchiveTocSide::Before) {
            items.reverse();
        }
        let next_cursor = if has_more {
            let edge = if matches!(side, SavedArchiveTocSide::Before) {
                items.first()
            } else {
                items.last()
            };
            edge.map(|item| {
                SavedArchiveTocCursor {
                    chapter_no: item.chapter_no,
                    chapter_id: item.chapter_id,
                }
                .encode()
            })
        } else {
            None
        };
        Ok(SavedArchiveTocPage { items, next_cursor })
    }
}
