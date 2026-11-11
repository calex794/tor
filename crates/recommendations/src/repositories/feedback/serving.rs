//! Safe catalogue fallback plus a separately versioned optional metadata/manual model.
use super::discovery::ACTOR_DISCOVERY_CTE;
use super::*;
use crate::domain::feedback::{Feed, FeedItem, MAX_ITEMS};
use sqlx::Row;

impl PgRecommendations {
    pub async fn feed(
        &self,
        actor: Option<i64>,
        seed: Option<i64>,
        limit: usize,
    ) -> Result<Feed, FeedbackError> {
        if !(1..=MAX_ITEMS).contains(&limit)
            || actor.is_some_and(|a| a <= 0)
            || seed.is_some_and(|a| a <= 0)
        {
            return Err(FeedbackError::Invalid);
        }
        // Explicit page context still needs live authorization. A retired
        // analytics model never turns an inaccessible seed into a public hint.
        if let Some(seed) = seed {
            let sql = format!(
                "{ACTOR_DISCOVERY_CTE} SELECT EXISTS(SELECT 1 FROM rec_actor_books WHERE id=$2)"
            );
            let allowed: bool = sqlx::query_scalar(&sql)
                .bind(actor)
                .bind(seed)
                .fetch_one(&self.pool)
                .await?;
            if !allowed {
                return Err(FeedbackError::NotFound);
            }
        }
        // Only an explicit visible page context can select the metadata/manual model.
        // Optional model storage contention must not block ordinary catalogue discovery.
        if let Some(seed) = seed {
            if let Ok(Ok(Some(feed))) = tokio::time::timeout(
                std::time::Duration::from_millis(400),
                self.contextual_feed(actor, seed, limit),
            )
            .await
            {
                return Ok(feed);
            }
        }
        // The actor is used only for current discovery rights and explicit
        // exclusions. Their absence does not create a preference or REC scope.
        let sql = format!(
            "{ACTOR_DISCOVERY_CTE}
             SELECT b.id,b.title,
               (COALESCE(p.feedback_epoch,0)*$4+COALESCE(f.revision,0)) AS revision
             FROM rec_actor_books b
             LEFT JOIN rec_preference_accounts p ON p.user_id=$1
             LEFT JOIN rec_book_preferences f ON f.user_id=p.user_id AND f.book_id=b.id
             WHERE ($2::bigint IS NULL OR b.id<>$2)
               AND NOT(COALESCE(f.hidden,false) AND f.expires_at>now())
             ORDER BY b.created_at DESC,b.id DESC LIMIT $3"
        );
        let rows = sqlx::query(&sql)
            .bind(actor)
            .bind(seed)
            .bind(limit as i64)
            .bind(REVISION_STRIDE)
            .fetch_all(&self.pool)
            .await?;
        let mut items = Vec::new();
        for row in rows {
            items.push(FeedItem {
                book_id: row.try_get::<i64, _>("id")?.to_string(),
                title: row.try_get("title")?,
                source: "catalog".to_owned(),
                reason: "new_release".to_owned(),
                algorithm_version: "catalog_new_releases_v1",
                ticket: None,
                feedback_revision: actor
                    .map(|_| row.try_get::<i64, _>("revision"))
                    .transpose()?,
            });
        }
        Ok(Feed {
            items,
            generation_id: None,
            next_cursor: None,
            // Explicit hides and access checks remain account-specific. This
            // flag means there is no ranking based on personal reading history.
            personalized: false,
        })
    }
}
