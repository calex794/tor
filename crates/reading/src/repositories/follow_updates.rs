//! Read-only, per-reader updates from canonical first-release facts.
//! This is an on-site feed, not an email/push delivery or a paid access grant.
#![forbid(unsafe_code)]
use sqlx::{PgPool, Row};

pub const UPDATES_PAGE_SIZE: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdatesCursor {
    pub released_micros: i64,
    pub chapter_id: i64,
}

impl UpdatesCursor {
    pub fn parse(raw: &str) -> Option<Self> {
        if raw.len() > 48 {
            return None;
        }
        let (micros, chapter) = raw.split_once('-')?;
        let released_micros = micros.parse::<i64>().ok()?;
        let chapter_id = chapter.parse::<i64>().ok()?;
        if released_micros <= 0 || chapter_id <= 0 {
            return None;
        }
        Some(Self {
            released_micros,
            chapter_id,
        })
    }

    pub fn encode(self) -> String {
        format!("{}-{}", self.released_micros, self.chapter_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowedRelease {
    pub book_id: i64,
    pub chapter_id: i64,
    pub book_title: String,
    pub chapter_title: String,
    pub chapter_number: i32,
    pub released_micros: i64,
}

#[derive(Debug, Clone)]
pub struct FollowedReleasePage {
    pub items: Vec<FollowedRelease>,
    pub next_cursor: Option<UpdatesCursor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowUpdatesError {
    Invalid,
    Unavailable,
}

#[derive(Clone)]
pub struct PgFollowUpdates {
    pool: PgPool,
}

impl PgFollowUpdates {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn latest(
        &self,
        actor: i64,
        before: Option<UpdatesCursor>,
    ) -> Result<FollowedReleasePage, FollowUpdatesError> {
        if actor <= 0 {
            return Err(FollowUpdatesError::Invalid);
        }
        // A new follow cannot retroactively subscribe to old releases.
        // A hidden/paid/delayed chapter never leaks through this free-only feed.
        // SQL pages the current reader only; there is no global 5k fan-out cap.
        let rows = sqlx::query(
            "SELECT book.id AS book_id, book.title AS book_title,
                    chapter.id AS chapter_id, chapter.title AS chapter_title,
                    chapter.chapter_no AS chapter_number,
                    (EXTRACT(EPOCH FROM release.created_at) * 1000000)::bigint
                        AS released_micros
             FROM reader_book_follows AS follow
             JOIN books AS book ON book.id=follow.book_id
             JOIN chapters AS chapter ON chapter.book_id=book.id
             JOIN chapter_first_release_events AS release
               ON release.chapter_id=chapter.id
             JOIN chapter_publications AS publication
               ON publication.chapter_id=chapter.id
             WHERE follow.user_id=$1
               AND release.created_at>=follow.created_at
               AND book.visibility='public'
               AND NOT book.admin_hold AND NOT book.is_deleted
               AND chapter.visibility='public'
               AND chapter.access_level='free' AND chapter.price_amount=0
               AND NOT chapter.admin_hold AND NOT chapter.is_deleted
               AND (chapter.reader_released_at IS NULL
                    OR chapter.reader_released_at<=now())
               AND ($2::bigint IS NULL OR
                 ((EXTRACT(EPOCH FROM release.created_at) * 1000000)::bigint,
                  release.chapter_id) < ($2::bigint,$3::bigint))
             ORDER BY release.created_at DESC, release.chapter_id DESC
             LIMIT 31",
        )
        .bind(actor)
        .bind(before.map(|cursor| cursor.released_micros))
        .bind(before.map(|cursor| cursor.chapter_id))
        .fetch_all(&self.pool)
        .await
        .map_err(|_| FollowUpdatesError::Unavailable)?;
        let mut items = rows
            .into_iter()
            .map(|row| {
                Ok(FollowedRelease {
                    book_id: row
                        .try_get("book_id")
                        .map_err(|_| FollowUpdatesError::Unavailable)?,
                    chapter_id: row
                        .try_get("chapter_id")
                        .map_err(|_| FollowUpdatesError::Unavailable)?,
                    book_title: row
                        .try_get("book_title")
                        .map_err(|_| FollowUpdatesError::Unavailable)?,
                    chapter_title: row
                        .try_get("chapter_title")
                        .map_err(|_| FollowUpdatesError::Unavailable)?,
                    chapter_number: row
                        .try_get("chapter_number")
                        .map_err(|_| FollowUpdatesError::Unavailable)?,
                    released_micros: row
                        .try_get("released_micros")
                        .map_err(|_| FollowUpdatesError::Unavailable)?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let has_more = items.len() > UPDATES_PAGE_SIZE;
        items.truncate(UPDATES_PAGE_SIZE);
        let next_cursor = if has_more {
            items.last().map(|last| UpdatesCursor {
                released_micros: last.released_micros,
                chapter_id: last.chapter_id,
            })
        } else {
            None
        };
        Ok(FollowedReleasePage { items, next_cursor })
    }
}

#[cfg(test)]
mod tests {
    use super::UpdatesCursor;

    #[test]
    fn cursor_is_bounded_and_strict() {
        let cursor = UpdatesCursor::parse("1783000000000000-14").unwrap();
        assert_eq!(cursor.encode(), "1783000000000000-14");
        for invalid in ["", "-1", "0-1", "100-0", "100--1", "100-1-extra", "ab-4"] {
            assert_eq!(UpdatesCursor::parse(invalid), None);
        }
    }
}
