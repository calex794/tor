//! B-12: private author chapter pages use a stable (chapter_no, id) cursor.
//! The repository always scopes the query to the authenticated book owner.
use super::PgAuthoring;
use crate::services::authoring::{
    AuthorError, ChapterAccess, OwnedChapterCursor, OwnedChapterPage, OwnedChapterSummary,
};
use sqlx::Row;

impl PgAuthoring {
    /// Suggest the next chapter number to the authenticated book owner.
    /// NEwBOO auto-numbers chapters; TOR keeps an editable numeric field but
    /// must not force an author to manually correct the default "1".
    ///
    /// Include soft-deleted chapters: TOR has a permanent unique(book_id,
    /// chapter_no) constraint, unlike NEwBOO's reusable deleted numbers.
    /// This is a display suggestion, not a reserved number; concurrent
    /// submissions are still protected by the creation transaction's 409.
    pub async fn next_owned_chapter_number(
        &self,
        user_id: i64,
        book_id: i64,
    ) -> Result<i32, AuthorError> {
        if user_id <= 0 || book_id <= 0 {
            return Err(AuthorError::Missing);
        }
        let next: Option<i64> = sqlx::query_scalar(
            "SELECT COALESCE((
                 SELECT c.chapter_no::BIGINT FROM chapters c
                 WHERE c.book_id=b.id ORDER BY c.chapter_no DESC LIMIT 1
             ),0)+1
             FROM books b
             WHERE b.id=$1 AND b.owner_user_id=$2
               AND NOT b.is_deleted AND NOT b.admin_hold",
        )
        .bind(book_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        i32::try_from(next.ok_or(AuthorError::Missing)?)
            .map_err(|_| AuthorError::Conflict)
    }

    /// Author UI requests one bounded page; no offset scan for long novels.
    pub async fn list_owned_chapters_page(
        &self,
        user_id: i64,
        book_id: i64,
        after: Option<OwnedChapterCursor>,
    ) -> Result<OwnedChapterPage, AuthorError> {
        self.list_owned_chapters_page_with_limit(user_id, book_id, after, 100)
            .await
    }

    /// The old internal list API retains its 200-row bound for compatibility.
    pub(super) async fn list_owned_chapters_page_with_limit(
        &self,
        user_id: i64,
        book_id: i64,
        after: Option<OwnedChapterCursor>,
        page_size: usize,
    ) -> Result<OwnedChapterPage, AuthorError> {
        if user_id <= 0 || book_id <= 0 {
            return Err(AuthorError::Missing);
        }
        if !(1..=200).contains(&page_size) {
            return Err(AuthorError::Invalid);
        }
        let (after_number, after_id) = match after {
            None => (0_i32, 0_i64),
            Some(cursor) if cursor.number > 0 && cursor.id > 0 => (cursor.number, cursor.id),
            Some(_) => return Err(AuthorError::Invalid),
        };
        let limit = i64::try_from(page_size + 1).map_err(|_| AuthorError::Invalid)?;
        let rows = sqlx::query(
            "SELECT c.id,c.chapter_no,c.title,c.access_level,c.visibility
             FROM chapters c JOIN books b ON b.id=c.book_id
             WHERE c.book_id=$1 AND b.owner_user_id=$2
               AND NOT c.is_deleted AND NOT b.is_deleted
               AND NOT c.admin_hold AND NOT b.admin_hold
               AND (c.chapter_no,c.id)>($3,$4)
             ORDER BY c.chapter_no,c.id LIMIT $5",
        )
        .bind(book_id)
        .bind(user_id)
        .bind(after_number)
        .bind(after_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let has_more = rows.len() > page_size;
        let chapters: Vec<OwnedChapterSummary> = rows
            .into_iter()
            .take(page_size)
            .map(|row| {
                let access: String = row
                    .try_get("access_level")
                    .map_err(|_| AuthorError::Unavailable)?;
                let access = match access.as_str() {
                    "free" => ChapterAccess::Free,
                    "paid" => ChapterAccess::Paid,
                    _ => return Err(AuthorError::Unavailable),
                };
                let visibility: String = row
                    .try_get("visibility")
                    .map_err(|_| AuthorError::Unavailable)?;
                let published = match visibility.as_str() {
                    "public" => true,
                    "draft" => false,
                    _ => return Err(AuthorError::Unavailable),
                };
                Ok(OwnedChapterSummary {
                    id: row.try_get("id").map_err(|_| AuthorError::Unavailable)?,
                    number: row
                        .try_get("chapter_no")
                        .map_err(|_| AuthorError::Unavailable)?,
                    title: row.try_get("title").map_err(|_| AuthorError::Unavailable)?,
                    access,
                    published,
                })
            })
            .collect::<Result<_, AuthorError>>()?;
        let next = if has_more {
            chapters.last().map(|chapter| OwnedChapterCursor {
                number: chapter.number,
                id: chapter.id,
            })
        } else {
            None
        };
        Ok(OwnedChapterPage { chapters, next })
    }
}
