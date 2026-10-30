//! PostgreSQL-indexed title suggestions; never disclose someone else's draft.
use super::PgAuthoring;
use crate::services::{
    authoring::AuthorError,
    title_duplicates::{TitleCandidate, MAX_SUGGESTIONS},
};
use sqlx::Row;
use std::collections::{hash_map::Entry, HashMap};

impl PgAuthoring {
    /// A small query against B's title alias indexes, not a scan of all books.
    /// A title match is advisory: identical names can refer to different works.
    pub async fn title_candidates(
        &self,
        actor: i64,
        proposed: &[String],
    ) -> Result<Vec<TitleCandidate>, AuthorError> {
        if actor <= 0 {
            return Err(AuthorError::Forbidden);
        }
        if proposed.len() > 5
            || proposed
                .iter()
                .any(|title| title.chars().count() > 200 || title.chars().any(char::is_control))
        {
            return Err(AuthorError::Invalid);
        }
        let mut best = HashMap::<i64, TitleCandidate>::new();
        for raw in proposed {
            if raw.trim().is_empty() {
                continue;
            }
            let rows = sqlx::query(
                "SELECT b.id, b.title, b.target_language,
                        BOOL_OR(a.normalized_title = translate(lower(normalize($1::text,NFKC)), 'ё', 'е')) AS exact,
                        MAX(similarity(a.normalized_title, translate(lower(normalize($1::text,NFKC)), 'ё', 'е'))) AS score
                 FROM book_title_aliases AS a
                 JOIN books AS b ON b.id = a.book_id
                 WHERE NOT b.is_deleted
                   AND ((b.visibility = 'public' AND NOT b.admin_hold) OR b.owner_user_id = $2)
                   AND (
                     a.normalized_title = translate(lower(normalize($1::text,NFKC)), 'ё', 'е')
                     OR a.normalized_title % translate(lower(normalize($1::text,NFKC)), 'ё', 'е')
                   )
                 GROUP BY b.id, b.title, b.target_language
                 ORDER BY exact DESC, score DESC, b.id
                 LIMIT 12",
            )
            .bind(raw.trim())
            .bind(actor)
            .fetch_all(&self.pool)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
            for row in rows {
                let value = TitleCandidate {
                    id: row.try_get("id").map_err(|_| AuthorError::Unavailable)?,
                    title: row.try_get("title").map_err(|_| AuthorError::Unavailable)?,
                    target_language: row
                        .try_get("target_language")
                        .map_err(|_| AuthorError::Unavailable)?,
                    exact: row.try_get("exact").map_err(|_| AuthorError::Unavailable)?,
                    score: row.try_get("score").map_err(|_| AuthorError::Unavailable)?,
                };
                match best.entry(value.id) {
                    Entry::Vacant(slot) => {
                        slot.insert(value);
                    }
                    Entry::Occupied(mut slot) => {
                        let previous = slot.get();
                        if (value.exact && !previous.exact)
                            || (value.exact == previous.exact && value.score > previous.score)
                        {
                            slot.insert(value);
                        }
                    }
                }
            }
        }
        let mut matches: Vec<_> = best.into_values().collect();
        matches.sort_by(|a, b| {
            b.exact
                .cmp(&a.exact)
                .then_with(|| b.score.total_cmp(&a.score))
                .then_with(|| a.id.cmp(&b.id))
        });
        matches.truncate(MAX_SUGGESTIONS);
        Ok(matches)
    }
}
