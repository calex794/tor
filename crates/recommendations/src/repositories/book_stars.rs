//! F-05: separate readers' 1..5 ratings of a work and its translation.
//! They are not personal pairwise tiers and never grant reading rights.
#![forbid(unsafe_code)]

use sqlx::{PgPool, Row};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StarKind {
    Work,
    Translation,
}

impl StarKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "work" => Some(Self::Work),
            "translation" => Some(Self::Translation),
            _ => None,
        }
    }

    fn wire(self) -> &'static str {
        match self {
            Self::Work => "work",
            Self::Translation => "translation",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StarRatingError {
    Invalid,
    NotEligible,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StarTotal {
    pub sum: i64,
    pub votes: i64,
}

impl StarTotal {
    pub fn display_average(self) -> Option<String> {
        if self.votes <= 0 || self.sum < self.votes || self.sum > 5 * self.votes {
            return None;
        }
        // Exact rounded hundredths, no floating point for public rating.
        let hundredths = (self.sum * 100 + self.votes / 2) / self.votes;
        Some(format!("{}.{:02}", hundredths / 100, hundredths % 100))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BookStars {
    pub title: String,
    pub work: StarTotal,
    pub translation: StarTotal,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UserStars {
    pub work: Option<i16>,
    pub translation: Option<i16>,
}

#[derive(Clone)]
pub struct PgBookStars {
    pool: PgPool,
}

impl PgBookStars {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Public aggregates only for a currently discoverable book. Each kind
    /// is independent; no hidden votes, private user ids or pairwise tiers.
    pub async fn visible_summary(
        &self,
        book_id: i64,
    ) -> Result<Option<BookStars>, StarRatingError> {
        if book_id <= 0 {
            return Err(StarRatingError::Invalid);
        }
        // Public visibility and both vote totals are evaluated from the
        // same PostgreSQL statement snapshot; never expose aggregates from
        // a later snapshot after a book became held or hidden.
        let row = sqlx::query(
            "SELECT b.title,
                COALESCE(SUM(r.rating::bigint) FILTER
                    (WHERE r.rating_kind='work'),0)::bigint AS work_sum,
                COUNT(*) FILTER
                    (WHERE r.rating_kind='work')::bigint AS work_votes,
                COALESCE(SUM(r.rating::bigint) FILTER
                    (WHERE r.rating_kind='translation'),0)::bigint AS translation_sum,
                COUNT(*) FILTER
                    (WHERE r.rating_kind='translation')::bigint AS translation_votes
             FROM books b
             LEFT JOIN user_book_star_ratings r ON r.book_id=b.id
             WHERE b.id=$1 AND b.visibility='public'
               AND NOT b.admin_hold AND NOT b.is_deleted
             GROUP BY b.id,b.title",
        )
        .bind(book_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| StarRatingError::Unavailable)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let summary = BookStars {
            title: row
                .try_get("title")
                .map_err(|_| StarRatingError::Unavailable)?,
            work: StarTotal {
                sum: row
                    .try_get("work_sum")
                    .map_err(|_| StarRatingError::Unavailable)?,
                votes: row
                    .try_get("work_votes")
                    .map_err(|_| StarRatingError::Unavailable)?,
            },
            translation: StarTotal {
                sum: row
                    .try_get("translation_sum")
                    .map_err(|_| StarRatingError::Unavailable)?,
                votes: row
                    .try_get("translation_votes")
                    .map_err(|_| StarRatingError::Unavailable)?,
            },
        };
        if summary.work.votes > 0 && summary.work.display_average().is_none()
            || summary.translation.votes > 0 && summary.translation.display_average().is_none()
        {
            return Err(StarRatingError::Unavailable);
        }
        Ok(Some(summary))
    }

    /// Only the session user may read these two choices. The actor is never
    /// taken from a query/body field.
    pub async fn own_ratings(
        &self,
        actor: i64,
        book_id: i64,
    ) -> Result<UserStars, StarRatingError> {
        if actor <= 0 || book_id <= 0 {
            return Err(StarRatingError::Invalid);
        }
        let rows = sqlx::query(
            "SELECT r.rating_kind, r.rating
             FROM user_book_star_ratings r
             JOIN books b ON b.id=r.book_id
             WHERE r.user_id=$1 AND r.book_id=$2
               AND b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted",
        )
        .bind(actor)
        .bind(book_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| StarRatingError::Unavailable)?;
        let mut own = UserStars::default();
        for row in rows {
            let kind: String = row
                .try_get("rating_kind")
                .map_err(|_| StarRatingError::Unavailable)?;
            let rating: i16 = row
                .try_get("rating")
                .map_err(|_| StarRatingError::Unavailable)?;
            if !(1..=5).contains(&rating) {
                return Err(StarRatingError::Unavailable);
            }
            match StarKind::parse(&kind) {
                Some(StarKind::Work) => own.work = Some(rating),
                Some(StarKind::Translation) => own.translation = Some(rating),
                None => return Err(StarRatingError::Unavailable),
            }
        }
        Ok(own)
    }

    /// Used only to explain disabled rating controls; set_rating rechecks.
    pub async fn eligible(&self, actor: i64, book_id: i64) -> Result<bool, StarRatingError> {
        if actor <= 0 || book_id <= 0 {
            return Err(StarRatingError::Invalid);
        }
        sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM books b
                WHERE b.id=$2 AND b.visibility='public'
                  AND NOT b.admin_hold AND NOT b.is_deleted
                  AND EXISTS(
                    SELECT 1 FROM reader_chapter_completions c
                    WHERE c.user_id=$1 AND c.book_id=$2
                  )
             )",
        )
        .bind(actor)
        .bind(book_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|_| StarRatingError::Unavailable)
    }

    /// One upsert per user/book/kind. Exact original value is stored, and
    /// aggregate COUNT/SUM are derived from rows (also after erasure).
    /// The canonical C completion is required, not an S3 GET or bookmark.
    pub async fn set_rating(
        &self,
        actor: i64,
        book_id: i64,
        kind: StarKind,
        rating: i16,
    ) -> Result<(), StarRatingError> {
        if actor <= 0 || book_id <= 0 || !(1..=5).contains(&rating) {
            return Err(StarRatingError::Invalid);
        }
        let result: Option<i16> = sqlx::query_scalar(
            "INSERT INTO user_book_star_ratings
                (user_id,book_id,rating_kind,rating)
             SELECT $1,$2,$3,$4
             WHERE EXISTS (
                 SELECT 1 FROM books b WHERE b.id=$2
                   AND b.visibility='public' AND NOT b.admin_hold
                   AND NOT b.is_deleted
             ) AND EXISTS (
                 SELECT 1 FROM reader_chapter_completions c
                 WHERE c.user_id=$1 AND c.book_id=$2
             )
             ON CONFLICT(user_id,book_id,rating_kind) DO UPDATE SET
                 rating=EXCLUDED.rating,
                 updated_at=CASE
                   WHEN user_book_star_ratings.rating <> EXCLUDED.rating THEN now()
                   ELSE user_book_star_ratings.updated_at END
             RETURNING rating",
        )
        .bind(actor)
        .bind(book_id)
        .bind(kind.wire())
        .bind(rating)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| StarRatingError::Unavailable)?;
        if result == Some(rating) {
            Ok(())
        } else {
            Err(StarRatingError::NotEligible)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f_05_work_and_translation_are_distinct_bounded_scores() {
        assert_eq!(StarKind::parse("work"), Some(StarKind::Work));
        assert_eq!(StarKind::parse("translation"), Some(StarKind::Translation));
        for invalid in ["rank", "overall", "WORK", ""] {
            assert_eq!(StarKind::parse(invalid), None);
        }
        assert_eq!(
            StarTotal { sum: 7, votes: 2 }.display_average(),
            Some("3.50".into())
        );
        assert_eq!(
            StarTotal { sum: 10, votes: 3 }.display_average(),
            Some("3.33".into())
        );
        assert_eq!(StarTotal { sum: 0, votes: 0 }.display_average(), None);
    }
}
