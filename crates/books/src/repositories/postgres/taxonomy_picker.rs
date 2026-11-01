//! Indexed B-owned taxonomy lookup, verified canonical selection and atomic links.
use super::*;
use crate::services::authoring::{AuthorError, TaxonomySelection};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct TaxonomySuggestion {
    pub id: i64,
    pub label: String,
    pub usage: i64,
}
#[derive(Debug, Clone, Default)]
pub struct TaxonomyNames {
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    pub fandoms: Vec<String>,
}

impl PgAuthoring {
    /// Publicly established vocabulary only; the caller authenticates first.
    /// A prefix lookup can use text_pattern_ops on canonical and alias keys.
    pub async fn suggest_taxonomies(
        &self,
        kind: &str,
        query: &str,
        limit: u8,
    ) -> Result<Vec<TaxonomySuggestion>, AuthorError> {
        if !matches!(kind, "genre" | "tag" | "fandom")
            || !(2..=60).contains(&query.chars().count())
            || query
                .chars()
                .any(|c| c.is_control() || matches!(c, '%' | '_' | '\\'))
        {
            return Err(AuthorError::Invalid);
        }
        if query.split_whitespace().collect::<String>().chars().count() < 2 {
            return Err(AuthorError::Invalid);
        }
        let rows = sqlx::query(
            "WITH prefix AS (
                SELECT book_taxonomy_key($2) AS k
            ), matches AS (
                SELECT t.id AS term_id,0 AS rank
                FROM book_taxonomy_terms t CROSS JOIN prefix p
                WHERE t.kind=$1 AND t.status='active'
                  AND char_length(p.k)>=2
                  -- NFKC can turn fullwidth punctuation into SQL wildcards.
                  AND strpos(p.k,'%')=0 AND strpos(p.k,'_')=0
                  AND strpos(p.k,chr(92))=0
                  AND t.search_key LIKE p.k || '%'
                UNION ALL
                SELECT a.term_id,1 AS rank
                FROM book_taxonomy_aliases a
                JOIN book_taxonomy_terms t ON t.id=a.term_id
                CROSS JOIN prefix p
                WHERE a.kind=$1 AND t.kind=$1 AND t.status='active'
                  AND char_length(p.k)>=2
                  AND strpos(p.k,'%')=0 AND strpos(p.k,'_')=0
                  AND strpos(p.k,chr(92))=0
                  AND a.search_key LIKE p.k || '%'
            ), ranked AS (
                SELECT term_id,min(rank) AS rank FROM matches GROUP BY term_id
            )
            SELECT t.id,t.label,coalesce(f.reference_count,0)::bigint AS usage
            FROM ranked r JOIN book_taxonomy_terms t ON t.id=r.term_id
            LEFT JOIN public_catalog_facets f
              ON f.kind=t.kind AND f.name=t.label
            WHERE t.status='active'
            ORDER BY r.rank ASC,usage DESC,t.label ASC,t.id ASC LIMIT $3",
        )
        .bind(kind)
        .bind(query)
        .bind(i64::from(limit.clamp(1, 15)))
        .fetch_all(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        rows.into_iter()
            .map(|row| {
                Ok(TaxonomySuggestion {
                    id: row.try_get("id").map_err(|_| AuthorError::Unavailable)?,
                    label: row.try_get("label").map_err(|_| AuthorError::Unavailable)?,
                    usage: row.try_get("usage").map_err(|_| AuthorError::Unavailable)?,
                })
            })
            .collect()
    }

    async fn names_for_ids(&self, kind: &str, ids: &[i64]) -> Result<Vec<String>, AuthorError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "SELECT id,search_key FROM book_taxonomy_terms
             WHERE kind=$1 AND status='active' AND id=ANY($2)",
        )
        .bind(kind)
        .bind(ids)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let mut names = HashMap::new();
        for row in rows {
            let id: i64 = row.try_get("id").map_err(|_| AuthorError::Unavailable)?;
            let name: String = row
                .try_get("search_key")
                .map_err(|_| AuthorError::Unavailable)?;
            names.insert(id, name);
        }
        ids.iter()
            .map(|id| names.get(id).cloned().ok_or(AuthorError::Invalid))
            .collect()
    }

    /// Names only come from active IDs, preserving the old arrays for F/REC.
    pub async fn resolve_taxonomy_selection(
        &self,
        selection: &TaxonomySelection,
    ) -> Result<TaxonomyNames, AuthorError> {
        Ok(TaxonomyNames {
            genres: self.names_for_ids("genre", &selection.genre_ids).await?,
            tags: self.names_for_ids("tag", &selection.tag_ids).await?,
            fandoms: self.names_for_ids("fandom", &selection.fandom_ids).await?,
        })
    }
}

pub(super) async fn persist_links(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    book_id: i64,
    book: &NewBook,
) -> Result<(), AuthorError> {
    // Legacy B API clients still provide normalized TEXT[] without IDs.
    // Their metadata remains intact; only new author HTTP forms require
    // canonical ID-backed selections. Mixed ID/text payloads fail closed.
    if book.taxonomy_ids.is_empty() {
        return Ok(());
    }
    let names = TaxonomyNames {
        genres: book.genres.clone(),
        tags: book.tags.clone(),
        fandoms: book.fandoms.clone(),
    };
    persist_selection_links(tx, book_id, &book.taxonomy_ids, &names).await
}

pub(super) async fn persist_selection_links(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    book_id: i64,
    selection: &TaxonomySelection,
    names: &TaxonomyNames,
) -> Result<(), AuthorError> {
    for (kind, ids, labels) in [
        ("genre", &selection.genre_ids, &names.genres),
        ("tag", &selection.tag_ids, &names.tags),
        ("fandom", &selection.fandom_ids, &names.fandoms),
    ] {
        if ids.len() != labels.len() || ids.len() > 8 {
            return Err(AuthorError::Invalid);
        }
        for (position, (id, name)) in ids.iter().zip(labels).enumerate() {
            // Guard against a forged ID, type mismatch, merged/blocked term or
            // a vocabulary change between lookup and this transaction.
            let inserted = sqlx::query(
                "INSERT INTO book_taxonomy_links(book_id,kind,term_id,position)
                 SELECT $1,t.kind,t.id,$3 FROM book_taxonomy_terms t
                 WHERE t.id=$2 AND t.kind=$4 AND t.status='active'
                   AND t.search_key=$5",
            )
            .bind(book_id)
            .bind(id)
            .bind(position as i16)
            .bind(kind)
            .bind(name)
            .execute(&mut **tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
            if inserted.rows_affected() != 1 {
                return Err(AuthorError::Invalid);
            }
        }
    }
    Ok(())
}
