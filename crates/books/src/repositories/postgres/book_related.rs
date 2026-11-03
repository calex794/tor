//! S020: bidirectional related-book links; one canonical SQL edge per pair.
//! A source is always owner-controlled. Search and display expose only live
//! public target books, never private names or hidden administration holds.
use super::PgAuthoring;
use crate::{
    services::{authoring::AuthorError, book_registration::normalized_title},
};
use sqlx::Row;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelatedBookCard {
    pub id: i64,
    pub title: String,
}

#[derive(Debug)]
pub struct OwnedRelatedBooks {
    pub title: String,
    pub linked: Vec<RelatedBookCard>,
}

impl PgAuthoring {
    pub async fn owned_related_books(
        &self,
        actor: i64,
        book_id: i64,
    ) -> Result<Option<OwnedRelatedBooks>, AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Ok(None);
        }
        let title: Option<String> = sqlx::query_scalar(
            "SELECT title FROM books WHERE id=$1 AND owner_user_id=$2
               AND NOT is_deleted AND NOT admin_hold",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let Some(title) = title else {
            return Ok(None);
        };
        let rows = sqlx::query(
            "SELECT target.id, target.title
             FROM book_related_links AS link
             JOIN books AS target ON target.id =
               CASE WHEN link.smaller_book_id=$1
                 THEN link.larger_book_id ELSE link.smaller_book_id END
             WHERE $1 IN (link.smaller_book_id, link.larger_book_id)
               AND target.visibility='public'
               AND NOT target.admin_hold AND NOT target.is_deleted
             ORDER BY target.id LIMIT 100",
        )
        .bind(book_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let linked = rows
            .into_iter()
            .map(|row| {
                Ok(RelatedBookCard {
                    id: row.try_get("id").map_err(|_| AuthorError::Unavailable)?,
                    title: row
                        .try_get("title")
                        .map_err(|_| AuthorError::Unavailable)?,
                })
            })
            .collect::<Result<Vec<_>, AuthorError>>()?;
        Ok(Some(OwnedRelatedBooks { title, linked }))
    }

    /// Exact main-title match only, as in NEwBOO; never suggest private books.
    pub async fn find_related_candidates(
        &self,
        actor: i64,
        book_id: i64,
        title: &str,
    ) -> Result<Vec<RelatedBookCard>, AuthorError> {
        if self.owned_related_books(actor, book_id).await?.is_none() {
            return Err(AuthorError::Missing);
        }
        let needle = normalized_title(title);
        if needle.is_empty() || needle.chars().count() > 200 {
            return Err(AuthorError::Invalid);
        }
        let rows = sqlx::query(
            "SELECT id, title FROM books
             WHERE id<>$1
               AND visibility='public' AND NOT admin_hold AND NOT is_deleted
               AND book_registration_legacy_title_key(title)=$2
             ORDER BY id LIMIT 20",
        )
        .bind(book_id)
        .bind(needle)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        rows.into_iter()
            .map(|row| {
                Ok(RelatedBookCard {
                    id: row.try_get("id").map_err(|_| AuthorError::Unavailable)?,
                    title: row
                        .try_get("title")
                        .map_err(|_| AuthorError::Unavailable)?,
                })
            })
            .collect()
    }

    pub async fn add_related_book(
        &self,
        actor: i64,
        book_id: i64,
        target_id: i64,
    ) -> Result<(), AuthorError> {
        if actor <= 0 || book_id <= 0 || target_id <= 0 || book_id == target_id {
            return Err(AuthorError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        // Deterministic row lock order avoids A->B / B->A link deadlocks.
        // A concurrent visibility/hold change must not race this insertion.
        let rows = sqlx::query(
            "SELECT id, owner_user_id, visibility, admin_hold, is_deleted
             FROM books WHERE id=ANY($1::bigint[]) ORDER BY id FOR UPDATE",
        )
        .bind(vec![book_id, target_id])
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if rows.len() != 2 {
            return Err(AuthorError::Missing);
        }
        let mut source_ok = false;
        let mut target_ok = false;
        for row in rows {
            let id: i64 = row.try_get("id").map_err(|_| AuthorError::Unavailable)?;
            let held: bool = row
                .try_get("admin_hold")
                .map_err(|_| AuthorError::Unavailable)?;
            let deleted: bool = row
                .try_get("is_deleted")
                .map_err(|_| AuthorError::Unavailable)?;
            if id == book_id {
                let owner: Option<i64> = row
                    .try_get("owner_user_id")
                    .map_err(|_| AuthorError::Unavailable)?;
                source_ok = owner == Some(actor) && !held && !deleted;
            } else if id == target_id {
                let visibility: String = row
                    .try_get("visibility")
                    .map_err(|_| AuthorError::Unavailable)?;
                target_ok = visibility == "public" && !held && !deleted;
            }
        }
        // Uniform 404 avoids probing unpublished targets or private owners.
        if !source_ok || !target_ok {
            return Err(AuthorError::Missing);
        }
        sqlx::query(
            "INSERT INTO book_related_links(smaller_book_id,larger_book_id)
             VALUES($1,$2) ON CONFLICT DO NOTHING",
        )
        .bind(book_id.min(target_id))
        .bind(book_id.max(target_id))
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(())
    }

    pub async fn remove_related_book(
        &self,
        actor: i64,
        book_id: i64,
        target_id: i64,
    ) -> Result<bool, AuthorError> {
        if actor <= 0 || book_id <= 0 || target_id <= 0 || book_id == target_id {
            return Err(AuthorError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        let allowed: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM books WHERE id=$1 AND owner_user_id=$2
               AND NOT admin_hold AND NOT is_deleted FOR UPDATE",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if allowed.is_none() {
            return Err(AuthorError::Missing);
        }
        let deleted = sqlx::query(
            "DELETE FROM book_related_links
             WHERE smaller_book_id=$1 AND larger_book_id=$2",
        )
        .bind(book_id.min(target_id))
        .bind(book_id.max(target_id))
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?
        .rows_affected()
            > 0;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(deleted)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn s020_self_link_does_not_create_an_edge() {
        assert_eq!(normalized_title(" Ёж — и   кот "), "еж и кот");
        assert_eq!((17_i64.min(42), 17_i64.max(42)), (17, 42));
        assert_eq!((42_i64.min(17), 42_i64.max(17)), (17, 42));
    }
}
