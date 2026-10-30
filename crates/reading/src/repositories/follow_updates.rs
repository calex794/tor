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
    /// This is storefront metadata, never a purchase or reading entitlement.
    pub is_paid: bool,
    /// Minor RUB units (kopecks). Zero for free chapters.
    pub price_minor: i64,
    /// Private per-reader acknowledgement, never inferred from page views.
    pub is_read: bool,
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
        // Include verified paid releases as purchase opportunities without
        // exposing paid content or granting any reading entitlement.
        // Unpublished, held and future-released chapters remain invisible.
        // SQL pages the current reader only; there is no global 5k fan-out cap.
        let rows = sqlx::query(
            "SELECT release.book_id,release.book_title,release.chapter_id,
                    release.chapter_title,release.chapter_number,
                    release.access_level,release.price_minor,release.released_micros,
                    (reads.read_at IS NOT NULL) AS is_read
             FROM reader_followed_release_candidates AS release
             LEFT JOIN reader_follow_notification_reads AS reads
               ON reads.user_id=release.user_id
              AND reads.chapter_id=release.chapter_id
             WHERE release.user_id=$1
               AND ($2::bigint IS NULL OR
                    (release.released_micros,release.chapter_id)<($2::bigint,$3::bigint))
             ORDER BY release.released_at DESC,release.chapter_id DESC
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
                    is_paid: row
                        .try_get::<String, _>("access_level")
                        .map_err(|_| FollowUpdatesError::Unavailable)?
                        == "paid",
                    price_minor: row
                        .try_get("price_minor")
                        .map_err(|_| FollowUpdatesError::Unavailable)?,
                    is_read: row
                        .try_get("is_read")
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

    /// Mark one visible release read/unread without changing the follow,
    /// library, purchases or read progress. Returns false for an ineligible
    /// chapter (including deleted, held and unpublished chapters).
    pub async fn set_read_state(
        &self,
        actor: i64,
        chapter_id: i64,
        read: bool,
    ) -> Result<bool, FollowUpdatesError> {
        if actor <= 0 || chapter_id <= 0 {
            return Err(FollowUpdatesError::Invalid);
        }
        let sql = if read {
            "WITH eligible AS (
                SELECT chapter_id FROM reader_followed_release_candidates
                WHERE user_id=$1 AND chapter_id=$2
             ), written AS (
                INSERT INTO reader_follow_notification_reads(user_id,chapter_id)
                SELECT $1,chapter_id FROM eligible
                ON CONFLICT(user_id,chapter_id) DO UPDATE SET read_at=now()
                RETURNING chapter_id
             )
             SELECT EXISTS(SELECT 1 FROM written)"
        } else {
            "WITH eligible AS (
                SELECT chapter_id FROM reader_followed_release_candidates
                WHERE user_id=$1 AND chapter_id=$2
             ), removed AS (
                DELETE FROM reader_follow_notification_reads AS reads
                WHERE reads.user_id=$1 AND reads.chapter_id=$2
                  AND EXISTS(SELECT 1 FROM eligible)
                RETURNING reads.chapter_id
             )
             SELECT EXISTS(SELECT 1 FROM eligible)
                AND (SELECT COUNT(*)>=0 FROM removed)"
        };
        sqlx::query_scalar::<_, bool>(sql)
            .bind(actor)
            .bind(chapter_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| FollowUpdatesError::Unavailable)
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
