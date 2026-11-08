//! C-03/H: bounded, bidirectional public chapter metadata around a verified chapter.
//! Every query repeats the B/age/publication gate; no chapter body is selected.
use super::{map_db, positive_id, PgReader};
use crate::services::{
    toc::{TocCursor, TocWindowPage, TocWindowPosition, MAX_PAGE_SIZE},
    PublicChapterLink, ReadingError,
};
use sqlx::{postgres::PgRow, Row};

// Keep the same publication and age predicates as public_book_page_for_session.
// $1 = book, $2 = chapter number/id, $3 = optional session digest.
const VISIBLE_CHAPTERS: &str = "\
    FROM chapters c \
    JOIN books b ON b.id = c.book_id \
    JOIN chapter_publications p ON p.chapter_id = c.id \
    JOIN chapter_versions v ON v.id = p.version_id AND v.chapter_id = c.id \
    WHERE c.book_id = $1 \
      AND b.visibility = 'public' AND b.visibility_status = 'active' \
      AND NOT b.admin_hold AND NOT b.is_deleted \
      AND c.visibility = 'public' AND NOT c.admin_hold AND NOT c.is_deleted \
      AND (c.reader_released_at IS NULL OR c.reader_released_at <= now()) \
      AND (b.access_level = 'basic' OR EXISTS ( \
          SELECT 1 FROM sessions sess JOIN users acct ON acct.id = sess.user_id \
          WHERE sess.token_hash = $3::bytea AND sess.revoked_at IS NULL \
            AND sess.expires_at > now() AND acct.account_state = 'active' \
            AND ((b.access_level = 'adult' AND acct.adult_enabled) \
              OR (b.access_level = 'pro_adult' AND EXISTS ( \
                  SELECT 1 FROM user_level_grants ag \
                  WHERE ag.user_id = acct.id AND ag.level = 'pro_adult' \
                    AND ag.is_active))) \
      ))";

const CHAPTER_FIELDS: &str = "SELECT c.id, c.chapter_no, c.title, c.access_level, c.price_amount ";

fn chapter_from_row(row: PgRow) -> Result<PublicChapterLink, ReadingError> {
    Ok(PublicChapterLink {
        id: positive_id(row.try_get("id").map_err(map_db)?)?,
        number: row.try_get("chapter_no").map_err(map_db)?,
        title: row.try_get("title").map_err(map_db)?,
        is_paid: row.try_get::<String, _>("access_level").map_err(map_db)? == "paid",
        price_minor: row.try_get("price_amount").map_err(map_db)?,
    })
}

impl PgReader {
    async fn visible_toc_anchor(
        &self,
        book_id: i64,
        chapter_id: i64,
        digest: Option<&[u8; 32]>,
    ) -> Result<Option<PublicChapterLink>, ReadingError> {
        let statement = format!("{CHAPTER_FIELDS}{VISIBLE_CHAPTERS} AND c.id = $2");
        sqlx::query(&statement)
            .bind(book_id)
            .bind(chapter_id)
            .bind(digest.map(|value| value.as_slice()))
            .fetch_optional(&self.pool)
            .await
            .map_err(map_db)?
            .map(chapter_from_row)
            .transpose()
    }

    // A reverse SQL query is reversed in memory so the browser always receives
    // ascending (chapter_no, id) order, including equal-numbered chapters.
    async fn visible_toc_side(
        &self,
        book_id: i64,
        anchor: TocCursor,
        before: bool,
        limit: u16,
        digest: Option<&[u8; 32]>,
    ) -> Result<(Vec<PublicChapterLink>, bool), ReadingError> {
        let (comparison, direction) = if before { ("<", "DESC") } else { (">", "ASC") };
        let statement = format!(
            "{CHAPTER_FIELDS}{VISIBLE_CHAPTERS} \
             AND (c.chapter_no, c.id) {comparison} ($2, $4) \
             ORDER BY c.chapter_no {direction}, c.id {direction} LIMIT $5"
        );
        let rows = sqlx::query(&statement)
            .bind(book_id)
            .bind(anchor.chapter_no)
            .bind(digest.map(|value| value.as_slice()))
            .bind(anchor.chapter_id)
            .bind(i64::from(limit) + 1)
            .fetch_all(&self.pool)
            .await
            .map_err(map_db)?;
        let has_more = rows.len() > usize::from(limit);
        let mut chapters = rows
            .into_iter()
            .take(usize::from(limit))
            .map(chapter_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        if before {
            chapters.reverse();
        }
        Ok((chapters, has_more))
    }

    /// A public TOC window, centered on an eligible chapter or extended from a
    /// cursor already issued to the browser. Cursors never grant read access.
    pub async fn public_toc_window_for_session(
        &self,
        book_id: u64,
        position: TocWindowPosition,
        limit: u16,
        digest: Option<&[u8; 32]>,
    ) -> Result<Option<TocWindowPage>, ReadingError> {
        if limit == 0 || limit > MAX_PAGE_SIZE {
            return Err(ReadingError::InvalidPage);
        }
        let book_id = i64::try_from(book_id).map_err(|_| ReadingError::InvalidPage)?;
        if book_id <= 0 {
            return Err(ReadingError::InvalidPage);
        }
        let anchor_id = match position {
            TocWindowPosition::Around(id) => {
                i64::try_from(id).map_err(|_| ReadingError::InvalidPage)?
            }
            TocWindowPosition::Before(cursor) | TocWindowPosition::After(cursor) => {
                cursor.chapter_id
            }
        };
        if anchor_id <= 0 {
            return Err(ReadingError::InvalidPage);
        }
        let Some(anchor_chapter) = self.visible_toc_anchor(book_id, anchor_id, digest).await?
        else {
            return Ok(None);
        };
        let anchor = TocCursor {
            chapter_no: anchor_chapter.number,
            chapter_id: i64::try_from(anchor_chapter.id).map_err(|_| ReadingError::Unavailable)?,
        };
        if let TocWindowPosition::Before(cursor) | TocWindowPosition::After(cursor) = position {
            if cursor != anchor {
                return Err(ReadingError::InvalidPage);
            }
        }

        let (chapters, has_before, has_after, total_chapters) = match position {
            TocWindowPosition::Around(_) => {
                let before_limit = (limit - 1) / 2;
                let after_limit = limit - 1 - before_limit;
                let (mut older, has_before) = self
                    .visible_toc_side(book_id, anchor, true, before_limit, digest)
                    .await?;
                let (newer, has_after) = self
                    .visible_toc_side(book_id, anchor, false, after_limit, digest)
                    .await?;
                older.push(anchor_chapter);
                older.extend(newer);
                // Counts only published, visible metadata for an eligible book.
                // The initial request needs the count; subsequent pages do not.
                let statement = format!("SELECT COUNT(*) {}", VISIBLE_CHAPTERS.replace("$3", "$2"));
                let total: i64 = sqlx::query_scalar(&statement)
                    .bind(book_id)
                    .bind(digest.map(|value| value.as_slice()))
                    .fetch_one(&self.pool)
                    .await
                    .map_err(map_db)?;
                (older, has_before, has_after, Some(total))
            }
            TocWindowPosition::Before(_) => {
                let (older, has_more) = self
                    .visible_toc_side(book_id, anchor, true, limit, digest)
                    .await?;
                (older, has_more, true, None)
            }
            TocWindowPosition::After(_) => {
                let (newer, has_more) = self
                    .visible_toc_side(book_id, anchor, false, limit, digest)
                    .await?;
                (newer, true, has_more, None)
            }
        };
        let before_cursor = if has_before {
            chapters
                .first()
                .map(|chapter| TocCursor::encode(chapter.number, chapter.id))
        } else {
            None
        };
        let after_cursor = if has_after {
            chapters
                .last()
                .map(|chapter| TocCursor::encode(chapter.number, chapter.id))
        } else {
            None
        };
        Ok(Some(TocWindowPage {
            chapters,
            before_cursor,
            after_cursor,
            total_chapters,
        }))
    }
}
