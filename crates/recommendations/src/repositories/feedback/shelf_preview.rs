//! Small, actor-authorized public book summaries for the chapter-end hover card.
//! This is presentation data only; it never grants access or stores reader activity.
use super::discovery::ACTOR_DISCOVERY_CTE;
use super::{FeedbackError, PgRecommendations};
use sqlx::Row;
use std::time::Duration;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShelfCardPreview {
    pub book_id: i64,
    pub description: String,
    pub genres: Vec<String>,
    pub work_rating: Option<String>,
    pub published_chapters: i64,
}

fn average_tenths(sum: i64, votes: i64) -> Option<String> {
    if votes <= 0 || sum < votes || sum > votes.checked_mul(5)? {
        return None;
    }
    let tenths = sum.checked_mul(10)?.checked_add(votes / 2)? / votes;
    Some(format!("{}.{:01}", tenths / 10, tenths % 10))
}

impl PgRecommendations {
    /// Read metadata for at most five currently discoverable Books in one query.
    /// The same actor-scoped ACL used to select shelf candidates is reapplied.
    pub async fn shelf_card_previews(
        &self,
        actor: Option<i64>,
        ids: &[i64],
    ) -> Result<Vec<ShelfCardPreview>, FeedbackError> {
        if ids.len() > 5 || ids.iter().any(|id| *id <= 0) || actor.is_some_and(|id| id <= 0) {
            return Err(FeedbackError::Invalid);
        }
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let sql = format!(
            "{ACTOR_DISCOVERY_CTE}
             SELECT b.id, LEFT(b.description, 360) AS description, b.genres,
               COALESCE((SELECT SUM(r.rating::bigint) FROM user_book_star_ratings r
                         WHERE r.book_id=b.id AND r.rating_kind='work'), 0)::bigint AS rating_sum,
               (SELECT COUNT(*)::bigint FROM user_book_star_ratings r
                WHERE r.book_id=b.id AND r.rating_kind='work') AS rating_votes,
               (SELECT COUNT(*)::bigint FROM chapters c
                WHERE c.book_id=b.id AND c.visibility='public'
                  AND NOT c.admin_hold AND NOT c.is_deleted
                  AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
                  AND EXISTS (SELECT 1 FROM chapter_publications p WHERE p.chapter_id=c.id)
               ) AS published_chapters
             FROM rec_actor_books visible JOIN books b ON b.id=visible.id
             WHERE visible.id=ANY($2::bigint[])"
        );
        let rows = tokio::time::timeout(
            Duration::from_millis(180),
            sqlx::query(&sql)
                .bind(actor)
                .bind(ids)
                .fetch_all(&self.pool),
        )
        .await
        .map_err(|_| FeedbackError::Unavailable)??;
        rows.into_iter()
            .map(|row| {
                let votes: i64 = row.try_get("rating_votes")?;
                let sum: i64 = row.try_get("rating_sum")?;
                let genres: Vec<String> = row.try_get("genres")?;
                let published_chapters: i64 = row.try_get("published_chapters")?;
                Ok(ShelfCardPreview {
                    book_id: row.try_get("id")?,
                    description: row.try_get("description")?,
                    genres: genres
                        .into_iter()
                        .filter(|value| !value.trim().is_empty())
                        .take(3)
                        .map(|value| value.chars().take(40).collect())
                        .collect(),
                    work_rating: average_tenths(sum, votes),
                    published_chapters: published_chapters.max(0),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::average_tenths;

    #[test]
    fn no_votes_or_invalid_aggregate_never_mints_fake_stars() {
        assert_eq!(average_tenths(0, 0), None);
        assert_eq!(average_tenths(1, 2), None);
        assert_eq!(average_tenths(11, 2), None);
        assert_eq!(average_tenths(9, 2), Some("4.5".into()));
        assert_eq!(average_tenths(10, 2), Some("5.0".into()));
    }
}
