//! Reader-end 50/40/10 shelf: owner/team/other owners (legacy `site`), read-only.
//! Unlike paid advertising, the shelf has no financial transaction or
//! individual impressions. Every candidate passes live Book discovery ACL.
use super::discovery::ACTOR_DISCOVERY_CTE;
use super::*;
use crate::domain::feedback::{Feed, FeedItem};
use crate::services::reader_end_shelf::{select_shelf_with_policy, ShelfCandidate};
use sqlx::Row;
use std::collections::BTreeMap;

impl PgRecommendations {
    pub async fn reader_end_shelf(
        &self,
        actor: Option<i64>,
        source_book: i64,
        chapter: i64,
    ) -> Result<Feed, FeedbackError> {
        self.reader_end_shelf_excluding(actor, source_book, chapter, &[], 5)
            .await
    }

    /// Reserve up to four chapter-end positions for live editorial assignments.
    /// Exclusions are server-generated, never supplied by reader input.
    pub async fn reader_end_shelf_excluding(
        &self,
        actor: Option<i64>,
        source_book: i64,
        chapter: i64,
        excluded: &[i64],
        slots: usize,
    ) -> Result<Feed, FeedbackError> {
        if source_book <= 0
            || chapter <= 0
            || actor.is_some_and(|id| id <= 0)
            || !(1..=5).contains(&slots)
            || excluded.len() > 4
            || excluded.iter().any(|id| *id <= 0)
        {
            return Err(FeedbackError::Invalid);
        }
        // One fresh, bounded F-owned policy read per organic shelf. No
        // request-local caching can keep an obsolete administrator revision.
        // A missing or unavailable policy omits optional cards, never text.
        let share_policy = self.shelf_share_policy().await?;
        // Reuse the exact actor-scoped discoverability predicate of the
        // existing REC serving path. No private title/metadata is selected.
        // The three independently bounded branches protect the owner and team
        // pools from being crowded out by the much larger other-owner catalogue.
        let sql = format!(
            "{ACTOR_DISCOVERY_CTE},
            origin AS (
              SELECT b.id,b.owner_user_id,b.genres,b.tags,b.target_language
              FROM books b JOIN rec_actor_books access ON access.id=b.id
              WHERE b.id=$2 AND b.owner_user_id IS NOT NULL
            ),
            source_teams AS (
              SELECT tb.team_id FROM translation_team_books tb
              JOIN translation_teams t ON t.id=tb.team_id
              JOIN origin o ON o.id=tb.book_id
              WHERE tb.is_active AND
                (o.owner_user_id=t.owner_user_id OR EXISTS (
                  SELECT 1 FROM translation_team_members m
                  JOIN users member ON member.id=m.user_id AND member.account_state='active'
                  WHERE m.team_id=t.id AND m.user_id=o.owner_user_id
                ))
            ),
            favored AS (
              SELECT book.genres,book.tags
              FROM user_book_rank r
              JOIN rec_actor_books visible ON visible.id=r.book_id
              JOIN books book ON book.id=visible.id
              WHERE r.user_id=$1
              ORDER BY r.tier ASC,book.id ASC LIMIT 5
            ),
            interests AS (
              SELECT
                COALESCE((SELECT array_agg(DISTINCT g.genre)
                          FROM favored f CROSS JOIN LATERAL unnest(f.genres) AS g(genre)),
                         ARRAY[]::text[]) AS genres,
                COALESCE((SELECT array_agg(DISTINCT t.tag)
                          FROM favored f CROSS JOIN LATERAL unnest(f.tags) AS t(tag)),
                         ARRAY[]::text[]) AS tags
            ),
            author_pool AS (
              SELECT visible.id,visible.title,0::integer AS kind,
                     (CASE WHEN candidate.genres && o.genres THEN 400 ELSE 0 END
                      +CASE WHEN candidate.tags && o.tags THEN 300 ELSE 0 END
                      +CASE WHEN candidate.target_language=o.target_language THEN 100 ELSE 0 END)::bigint AS relevance,
                     (CASE WHEN candidate.genres && prefs.genres THEN 120 ELSE 0 END
                      +CASE WHEN candidate.tags && prefs.tags THEN 80 ELSE 0 END)::bigint AS preference_boost
              FROM rec_actor_books visible
              JOIN books candidate ON candidate.id=visible.id
              CROSS JOIN origin o CROSS JOIN interests prefs
              WHERE visible.id<>o.id AND NOT (visible.id=ANY($4::bigint[])) AND candidate.owner_user_id=o.owner_user_id
              ORDER BY relevance DESC, preference_boost DESC, visible.created_at DESC, visible.id DESC LIMIT 100
            ),
            teammate_pool AS (
              SELECT visible.id,visible.title,1::integer AS kind,
                     (CASE WHEN candidate.genres && o.genres THEN 400 ELSE 0 END
                      +CASE WHEN candidate.tags && o.tags THEN 300 ELSE 0 END
                      +CASE WHEN candidate.target_language=o.target_language THEN 100 ELSE 0 END)::bigint AS relevance,
                     (CASE WHEN candidate.genres && prefs.genres THEN 120 ELSE 0 END
                      +CASE WHEN candidate.tags && prefs.tags THEN 80 ELSE 0 END)::bigint AS preference_boost
              FROM rec_actor_books visible
              JOIN books candidate ON candidate.id=visible.id
              CROSS JOIN origin o CROSS JOIN interests prefs
              WHERE visible.id<>o.id AND NOT (visible.id=ANY($4::bigint[])) AND (o.owner_user_id IS NULL OR candidate.owner_user_id IS DISTINCT FROM o.owner_user_id)
                AND EXISTS (
                  SELECT 1 FROM translation_team_books tb
                  JOIN source_teams st ON st.team_id=tb.team_id
                  JOIN translation_teams t ON t.id=tb.team_id
                  WHERE tb.book_id=candidate.id AND tb.is_active
                    AND (candidate.owner_user_id=t.owner_user_id OR EXISTS (
                      SELECT 1 FROM translation_team_members m
                      JOIN users member ON member.id=m.user_id AND member.account_state='active'
                      WHERE m.team_id=t.id AND m.user_id=candidate.owner_user_id
                    ))
                )
              ORDER BY relevance DESC, preference_boost DESC, visible.created_at DESC, visible.id DESC LIMIT 100
            ),
            site_pool AS (
              SELECT visible.id,visible.title,2::integer AS kind,
                     (CASE WHEN candidate.genres && o.genres THEN 400 ELSE 0 END
                      +CASE WHEN candidate.tags && o.tags THEN 300 ELSE 0 END
                      +CASE WHEN candidate.target_language=o.target_language THEN 100 ELSE 0 END)::bigint AS relevance,
                     (CASE WHEN candidate.genres && prefs.genres THEN 120 ELSE 0 END
                      +CASE WHEN candidate.tags && prefs.tags THEN 80 ELSE 0 END)::bigint AS preference_boost
              FROM rec_actor_books visible
              JOIN books candidate ON candidate.id=visible.id
              CROSS JOIN origin o CROSS JOIN interests prefs
              WHERE visible.id<>o.id AND NOT (visible.id=ANY($4::bigint[])) AND candidate.owner_user_id IS NOT NULL
                AND candidate.owner_user_id IS DISTINCT FROM o.owner_user_id
                AND NOT EXISTS (
                  SELECT 1 FROM translation_team_books tb
                  JOIN source_teams st ON st.team_id=tb.team_id
                  JOIN translation_teams t ON t.id=tb.team_id
                  WHERE tb.book_id=candidate.id AND tb.is_active
                    AND (candidate.owner_user_id=t.owner_user_id OR EXISTS (
                      SELECT 1 FROM translation_team_members m
                      JOIN users member ON member.id=m.user_id AND member.account_state='active'
                      WHERE m.team_id=t.id AND m.user_id=candidate.owner_user_id
                    ))
                )
              ORDER BY relevance DESC, preference_boost DESC, visible.created_at DESC, visible.id DESC LIMIT 100
            )
            SELECT candidate.id,candidate.title,candidate.kind,candidate.relevance,candidate.preference_boost,
                   (COALESCE(p.feedback_epoch,0)*$3+COALESCE(f.revision,0)) AS revision
            FROM (SELECT * FROM author_pool UNION ALL SELECT * FROM teammate_pool
                  UNION ALL SELECT * FROM site_pool) candidate
            LEFT JOIN rec_preference_accounts p ON p.user_id=$1
            LEFT JOIN rec_book_preferences f ON f.user_id=p.user_id AND f.book_id=candidate.id
            WHERE NOT(COALESCE(f.hidden,false) AND f.expires_at>now())"
        );
        let rows = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            sqlx::query(&sql)
                .bind(actor)
                .bind(source_book)
                .bind(REVISION_STRIDE)
                .bind(excluded)
                .fetch_all(&self.pool),
        )
        .await
        .map_err(|_| FeedbackError::Unavailable)??;
        let mut pools: [Vec<ShelfCandidate>; 3] = std::array::from_fn(|_| Vec::new());
        let mut metadata = BTreeMap::new();
        for row in rows {
            let kind: i32 = row.try_get("kind")?;
            let Ok(index) = usize::try_from(kind) else {
                continue;
            };
            if index >= pools.len() {
                continue;
            }
            let id: i64 = row.try_get("id")?;
            if id <= 0 {
                continue;
            }
            metadata.insert(
                id,
                (
                    row.try_get::<String, _>("title")?,
                    actor
                        .map(|_| row.try_get::<i64, _>("revision"))
                        .transpose()?,
                ),
            );
            pools[index].push(ShelfCandidate {
                book_id: id,
                relevance: row.try_get("relevance")?,
                preference_boost: row.try_get("preference_boost")?,
            });
        }
        // Each request rotates the bounded 5-card allocation independently;
        // the chapter is a public context, not an actor or tracking key.
        // A fixed rotation per chapter would skew quotas when a popular
        // first chapter is opened many times.
        let rotation = (chapter as u64) ^ rand::random::<u64>();
        let cards = select_shelf_with_policy(source_book, pools, slots, rotation, &share_policy.policy);
        let items = cards
            .into_iter()
            .filter_map(|card| {
                let (title, revision) = metadata.get(&card.book_id)?.clone();
                let source = match card.source {
                    crate::services::reader_end_shelf::ShelfSource::Author => "author",
                    crate::services::reader_end_shelf::ShelfSource::Teammate => "teammate",
                    crate::services::reader_end_shelf::ShelfSource::OtherAuthors => "site", // legacy wire name
                };
                Some(FeedItem {
                    book_id: card.book_id.to_string(),
                    title,
                    source: source.to_owned(),
                    reason: "similar_by_content".to_owned(),
                    algorithm_version: "reader_end_shelf_504010_v1",
                    ticket: None,
                    feedback_revision: revision,
                })
            })
            .collect();
        Ok(Feed {
            items,
            generation_id: None,
            next_cursor: None,
            personalized: false,
        })
    }
}
