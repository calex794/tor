//! A bounded optional context read; failure falls back to the ordinary live catalogue.
use super::super::discovery::ACTOR_DISCOVERY_CTE;
use super::*;
use crate::domain::feedback::{Feed, FeedItem};

impl PgRecommendations {
    pub(in crate::repositories::feedback) async fn contextual_feed(
        &self,
        actor: Option<i64>,
        seed: i64,
        limit: usize,
    ) -> Result<Option<Feed>, FeedbackError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION READ ONLY")
            .execute(&mut *tx)
            .await?;
        sqlx::query("SET LOCAL lock_timeout='75ms'")
            .execute(&mut *tx)
            .await?;
        sqlx::query("SET LOCAL statement_timeout='300ms'")
            .execute(&mut *tx)
            .await?;
        let sql=format!("{ACTOR_DISCOVERY_CTE}, active AS (
          SELECT g.id FROM rec_runtime_policy p JOIN rec_generations g ON g.id=p.active_generation
          WHERE p.singleton AND NOT p.hold AND p.rollout_percent=100 AND g.model_version=$5
            AND g.status='validated' AND g.finished_at IS NOT NULL AND octet_length(g.manifest_hash)=32
        ), choices AS (
          SELECT b.id,b.title,e.source,e.reason,e.score,a.id::text AS generation,
            (COALESCE(p.feedback_epoch,0)*$4+COALESCE(f.revision,0)) AS revision,
            row_number() OVER(PARTITION BY b.id ORDER BY (e.source='related_books') DESC,e.score DESC) AS choice
          FROM active a JOIN rec_candidate_edges e ON e.generation_id=a.id AND e.seed_id=$2
          JOIN rec_actor_books b ON b.id=e.book_id
          LEFT JOIN rec_preference_accounts p ON p.user_id=$1
          LEFT JOIN rec_book_preferences f ON f.user_id=p.user_id AND f.book_id=b.id
          WHERE e.book_id<>$2 AND EXISTS(SELECT 1 FROM rec_actor_books WHERE id=$2)
            AND NOT(COALESCE(f.hidden,false) AND f.expires_at>now())
            AND ((e.source='content' AND e.reason='similar_by_content' AND EXISTS(
                SELECT 1 FROM books seed JOIN books candidate ON candidate.id=b.id
                WHERE seed.id=$2 AND seed.target_language=candidate.target_language
                  AND (seed.genres && candidate.genres OR seed.tags && candidate.tags)))
              OR (e.source='related_books' AND e.reason='manually_related' AND EXISTS(
                SELECT 1 FROM book_recommendation_relations link
                  WHERE link.book_id=$2 AND link.related_book_id=b.id)))
        ) SELECT * FROM choices WHERE choice=1
          ORDER BY (source='related_books') DESC,score DESC,id::text COLLATE \"C\" LIMIT $3");
        let rows = sqlx::query(&sql)
            .bind(actor)
            .bind(seed)
            .bind(limit as i64)
            .bind(REVISION_STRIDE)
            .bind(MODEL)
            .fetch_all(&mut *tx)
            .await?;
        if rows.is_empty() {
            tx.commit().await?;
            return Ok(None);
        }
        let generation = rows[0].try_get("generation")?;
        let items = rows
            .into_iter()
            .map(|r| {
                Ok(FeedItem {
                    book_id: r.try_get::<i64, _>("id")?.to_string(),
                    title: r.try_get("title")?,
                    source: r.try_get("source")?,
                    reason: r.try_get("reason")?,
                    algorithm_version: MODEL,
                    ticket: None,
                    feedback_revision: actor.map(|_| r.try_get("revision")).transpose()?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        tx.commit().await?;
        Ok(Some(Feed {
            items,
            generation_id: Some(generation),
            next_cursor: None,
            personalized: false,
        }))
    }
}
