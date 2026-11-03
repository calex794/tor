//! Public-only, indexed title hints for S019. Never query private book names.
use super::PgAuthoring;
use crate::services::{authoring::AuthorError, title_suggestions};
use sqlx::Row;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicTitleSuggestion {
    pub id: i64,
    pub title: String,
    pub language: String,
}

impl PgAuthoring {
    /// Advisory suggestions only; create_owned_book retains its atomic guards.
    pub async fn public_title_suggestions(
        &self,
        language: &str,
        titles: &[&str],
    ) -> Result<Vec<PublicTitleSuggestion>, AuthorError> {
        let language = title_suggestions::target_language(language)?;
        let terms = title_suggestions::normalize_terms(titles)?;
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        // One trigram-indexed search against PUBLIC names only; aliases from
        // historical books and newly registered books use the same projection.
        // The raw matched alias is never returned, only the public book card.
        let rows = sqlx::query(
            "SELECT b.id, b.title, b.target_language
             FROM unnest($1::text[]) AS search(term)
             JOIN books AS b
               ON b.visibility='public' AND b.visibility_status='active' AND NOT b.admin_hold AND NOT b.is_deleted
              AND b.target_language=$2
              AND (
                search.term <% book_registration_public_title_text(
                    b.title, b.original_title, b.alt_titles
                )
                OR book_registration_public_title_text(
                    b.title, b.original_title, b.alt_titles
                ) ILIKE ('%' || search.term || '%')
              )
             GROUP BY b.id, b.title, b.target_language
             ORDER BY MAX(word_similarity(
                 search.term,
                 book_registration_public_title_text(
                     b.title, b.original_title, b.alt_titles
                 )
             )) DESC, b.id DESC
             LIMIT $3",
        )
        .bind(&terms)
        .bind(language)
        .bind(title_suggestions::MAX_SUGGESTIONS)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        rows.into_iter()
            .map(|row| {
                Ok(PublicTitleSuggestion {
                    id: row.try_get("id").map_err(|_| AuthorError::Unavailable)?,
                    title: row.try_get("title").map_err(|_| AuthorError::Unavailable)?,
                    language: row
                        .try_get("target_language")
                        .map_err(|_| AuthorError::Unavailable)?,
                })
            })
            .collect()
    }
}
