//! B/TAX-02: owner-only canonical taxonomy edits, revision fenced and atomic.
use super::*;
use crate::services::authoring::{AuthorError, TaxonomySelection};
use super::taxonomy_picker::{self, TaxonomyNames};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct TaxonomyEntry {
    pub id: i64,
    pub label: String,
    pub search_key: String,
    pub active: bool,
}

#[derive(Default, Debug)]
struct Groups {
    genres: Vec<TaxonomyEntry>,
    tags: Vec<TaxonomyEntry>,
    fandoms: Vec<TaxonomyEntry>,
}

#[derive(Debug)]
pub struct TaxonomySnapshot {
    pub revision: i64,
    pub genres: Vec<TaxonomyEntry>,
    pub tags: Vec<TaxonomyEntry>,
    pub fandoms: Vec<TaxonomyEntry>,
    pub raw_genres: Vec<String>,
    pub raw_tags: Vec<String>,
    pub raw_fandoms: Vec<String>,
    /// Refuse to silently erase legacy labels absent from the active dictionary.
    pub editable: bool,
}

fn groups_from_rows(rows: Vec<PgRow>) -> Result<Groups, AuthorError> {
    let mut result = Groups::default();
    for row in rows {
        let item = TaxonomyEntry {
            id: row.try_get("term_id").map_err(|_| AuthorError::Unavailable)?,
            label: row.try_get("label").map_err(|_| AuthorError::Unavailable)?,
            search_key: row.try_get("search_key").map_err(|_| AuthorError::Unavailable)?,
            active: row.try_get::<String, _>("status")
                .map_err(|_| AuthorError::Unavailable)? == "active",
        };
        match row.try_get::<&str, _>("kind").map_err(|_| AuthorError::Unavailable)? {
            "genre" => result.genres.push(item),
            "tag" => result.tags.push(item),
            "fandom" => result.fandoms.push(item),
            _ => return Err(AuthorError::Unavailable),
        }
    }
    Ok(result)
}

fn projection_matches(
    genres: &[String],
    tags: &[String],
    fandoms: &[String],
    groups: &Groups,
) -> bool {
    fn same(labels: &[String], items: &[TaxonomyEntry]) -> bool {
        labels.len() == items.len()
            && labels.iter().zip(items.iter()).all(|(label, item)| {
                item.active && label == &item.search_key
            })
    }
    same(genres, &groups.genres)
        && same(tags, &groups.tags)
        && same(fandoms, &groups.fandoms)
}

const LINKS_QUERY: &str = "
    SELECT l.kind,l.term_id,t.label,t.search_key,t.status
    FROM book_taxonomy_links l JOIN book_taxonomy_terms t
      ON t.kind=l.kind AND t.id=l.term_id
    WHERE l.book_id=$1
    ORDER BY l.kind,l.position
";

async fn names_for_ids(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    kind: &str,
    ids: &[i64],
) -> Result<Vec<String>, AuthorError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "SELECT id,search_key FROM book_taxonomy_terms
         WHERE kind=$1 AND status='active' AND id=ANY($2)",
    )
    .bind(kind)
    .bind(ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    let mut names = HashMap::new();
    for row in rows {
        names.insert(
            row.try_get::<i64, _>("id").map_err(|_| AuthorError::Unavailable)?,
            row.try_get::<String, _>("search_key").map_err(|_| AuthorError::Unavailable)?,
        );
    }
    ids.iter()
        .map(|id| names.get(id).cloned().ok_or(AuthorError::Invalid))
        .collect()
}

impl PgAuthoring {
    pub async fn taxonomy_snapshot(
        &self,
        actor: i64,
        book_id: i64,
    ) -> Result<Option<TaxonomySnapshot>, AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT taxonomy_revision,genres,tags,fandoms
             FROM books WHERE id=$1 AND owner_user_id=$2
               AND NOT admin_hold AND NOT is_deleted",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let Some(row) = row else { return Ok(None) };
        let revision: i64 = row.try_get("taxonomy_revision").map_err(|_| AuthorError::Unavailable)?;
        let raw_genres: Vec<String> = row.try_get("genres").map_err(|_| AuthorError::Unavailable)?;
        let raw_tags: Vec<String> = row.try_get("tags").map_err(|_| AuthorError::Unavailable)?;
        let raw_fandoms: Vec<String> = row.try_get("fandoms").map_err(|_| AuthorError::Unavailable)?;
        let rows = sqlx::query(LINKS_QUERY).bind(book_id).fetch_all(&self.pool).await
            .map_err(|_| AuthorError::Unavailable)?;
        let groups = groups_from_rows(rows)?;
        let editable = projection_matches(&raw_genres, &raw_tags, &raw_fandoms, &groups);
        Ok(Some(TaxonomySnapshot {
            revision,
            genres: groups.genres,
            tags: groups.tags,
            fandoms: groups.fandoms,
            raw_genres,
            raw_tags,
            raw_fandoms,
            editable,
        }))
    }

    /// Locks the owner row, compares revision and old array/link projection,
    /// resolves active selected IDs, replaces links and arrays in one TX.
    pub async fn update_book_taxonomy(
        &self,
        actor: i64,
        book_id: i64,
        expected_revision: i64,
        selection: &TaxonomySelection,
    ) -> Result<(), AuthorError> {
        if actor <= 0 {
            return Err(AuthorError::Forbidden);
        }
        if book_id <= 0 {
            return Err(AuthorError::Missing);
        }
        if expected_revision <= 0 {
            return Err(AuthorError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(|_| AuthorError::Unavailable)?;
        let row = sqlx::query(
            "SELECT taxonomy_revision,genres,tags,fandoms FROM books
             WHERE id=$1 AND owner_user_id=$2
               AND NOT admin_hold AND NOT is_deleted FOR UPDATE",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?
        .ok_or(AuthorError::Missing)?;
        let revision: i64 = row.try_get("taxonomy_revision").map_err(|_| AuthorError::Unavailable)?;
        if revision != expected_revision {
            return Err(AuthorError::Conflict);
        }
        let old_genres: Vec<String> = row.try_get("genres").map_err(|_| AuthorError::Unavailable)?;
        let old_tags: Vec<String> = row.try_get("tags").map_err(|_| AuthorError::Unavailable)?;
        let old_fandoms: Vec<String> = row.try_get("fandoms").map_err(|_| AuthorError::Unavailable)?;
        let links = sqlx::query(LINKS_QUERY).bind(book_id).fetch_all(&mut *tx).await
            .map_err(|_| AuthorError::Unavailable)?;
        let current = groups_from_rows(links)?;
        if !projection_matches(&old_genres, &old_tags, &old_fandoms, &current) {
            return Err(AuthorError::Conflict);
        }
        if selection.genre_ids == current.genres.iter().map(|t| t.id).collect::<Vec<_>>()
            && selection.tag_ids == current.tags.iter().map(|t| t.id).collect::<Vec<_>>()
            && selection.fandom_ids == current.fandoms.iter().map(|t| t.id).collect::<Vec<_>>()
        {
            return Ok(()); // No-op does not invalidate recommendation surfaces.
        }
        let names = TaxonomyNames {
            genres: names_for_ids(&mut tx, "genre", &selection.genre_ids).await?,
            tags: names_for_ids(&mut tx, "tag", &selection.tag_ids).await?,
            fandoms: names_for_ids(&mut tx, "fandom", &selection.fandom_ids).await?,
        };
        // Every update of published arrays also updates the existing F facet
        // projections through their established books UPDATE triggers.
        sqlx::query(
            "UPDATE books SET genres=$3,tags=$4,fandoms=$5,
               taxonomy_revision=taxonomy_revision+1
             WHERE id=$1 AND owner_user_id=$2 AND taxonomy_revision=$6",
        )
        .bind(book_id)
        .bind(actor)
        .bind(&names.genres)
        .bind(&names.tags)
        .bind(&names.fandoms)
        .bind(expected_revision)
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        sqlx::query("DELETE FROM book_taxonomy_links WHERE book_id=$1")
            .bind(book_id).execute(&mut *tx).await.map_err(|_| AuthorError::Unavailable)?;
        taxonomy_picker::persist_selection_links(&mut tx, book_id, selection, &names).await?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(())
    }
}
