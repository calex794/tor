//! F-owned persisted share policy for organic reader-end Books.
//! A policy read never collects viewer activity or grants access to a Book.
use super::*;
use crate::services::reader_end_shelf::ShelfSharePolicy;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShelfShareSnapshot {
    pub revision: i64,
    pub policy: ShelfSharePolicy,
}

impl PgRecommendations {
    /// One authoritative version per request; explicit 0% is never replaced by
    /// stale defaults. The administrator CAS owns changes, not this read path.
    pub async fn shelf_share_policy(&self) -> Result<ShelfShareSnapshot, FeedbackError> {
        let values: (i64, i16, i16, i16, i16, i16) = tokio::time::timeout(
            Duration::from_millis(500),
            sqlx::query_as(
                "SELECT revision, author_pct, teammate_pct, other_authors_pct,
                        no_team_author_pct, no_team_other_authors_pct
                 FROM rec_shelf_share_policy WHERE singleton",
            )
            .fetch_one(&self.pool),
        )
        .await
        .map_err(|_| FeedbackError::Unavailable)??;
        let policy = ShelfSharePolicy::validated(
            [
                i32::from(values.1),
                i32::from(values.2),
                i32::from(values.3),
            ],
            [i32::from(values.4), i32::from(values.5)],
        )
        .map_err(|_| FeedbackError::Unavailable)?;
        Ok(ShelfShareSnapshot {
            revision: values.0,
            policy,
        })
    }
}
