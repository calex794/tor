//! S020: bidirectional related-book links; one canonical SQL edge per pair.
//! A source is controlled by an active Book member. Search and display expose only live
//! public target books, never private names or hidden administration holds.
use super::{PgAuthoring, PgCatalog};
use crate::services::{authoring::AuthorError, book_registration::normalized_title};
use sqlx::{Postgres, Row, Transaction};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelatedBookCard {
    pub id: i64,
    pub title: String,
}

#[derive(Debug)]
pub struct ManagedRelatedBooks {
    pub title: String,
    pub linked: Vec<RelatedBookCard>,
}

/// Read the actor's active Book membership only after the source Book row is
/// locked. This closes the TOCTOU window with an owner's collaborator revoke:
/// both operations lock Book first, then BookMember. FOR SHARE also serializes
/// direct membership changes against this transaction.
pub(super) async fn lock_manageable_member(
    tx: &mut Transaction<'_, Postgres>,
    actor: i64,
    book_id: i64,
) -> Result<(), AuthorError> {
    let role: Option<String> = sqlx::query_scalar(
        "SELECT m.role FROM book_members m
         JOIN books b ON b.id=m.book_id
         WHERE m.book_id=$1 AND m.user_id=$2 AND m.is_active
           AND (m.role IN ('translator','moderator')
                OR (m.role='owner' AND b.owner_user_id=$2))
         FOR SHARE OF m",
    )
    .bind(book_id)
    .bind(actor)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    role.ok_or(AuthorError::Missing)?;
    Ok(())
}

impl PgAuthoring {
    pub async fn manageable_related_books(
        &self,
        actor: i64,
        book_id: i64,
    ) -> Result<Option<ManagedRelatedBooks>, AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Ok(None);
        }
        let title: Option<String> = sqlx::query_scalar(
            "SELECT b.title FROM books b WHERE b.id=$1
               AND NOT b.is_deleted AND NOT b.admin_hold
               AND EXISTS (
                 SELECT 1 FROM book_members m
                 WHERE m.book_id=b.id AND m.user_id=$2 AND m.is_active
                   AND (m.role IN ('translator','moderator')
                        OR (m.role='owner' AND b.owner_user_id=$2))
               )",
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
               AND target.visibility='public' AND target.visibility_status='active'
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
                    title: row.try_get("title").map_err(|_| AuthorError::Unavailable)?,
                })
            })
            .collect::<Result<Vec<_>, AuthorError>>()?;
        Ok(Some(ManagedRelatedBooks { title, linked }))
    }

    /// Exact main-title match only, as in NEwBOO; never suggest private books.
    pub async fn find_related_candidates(
        &self,
        actor: i64,
        book_id: i64,
        title: &str,
    ) -> Result<Vec<RelatedBookCard>, AuthorError> {
        if self
            .manageable_related_books(actor, book_id)
            .await?
            .is_none()
        {
            return Err(AuthorError::Missing);
        }
        let needle = normalized_title(title);
        if needle.is_empty() || needle.chars().count() > 200 {
            return Err(AuthorError::Invalid);
        }
        let rows = sqlx::query(
            "SELECT id, title FROM books
             WHERE id<>$1
               AND visibility='public' AND visibility_status='active' AND NOT admin_hold AND NOT is_deleted
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
                    title: row.try_get("title").map_err(|_| AuthorError::Unavailable)?,
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
            "SELECT id, visibility, visibility_status, admin_hold, is_deleted
             FROM books WHERE id=ANY($1::bigint[])
             ORDER BY id FOR UPDATE",
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
                source_ok = !held && !deleted;
            } else if id == target_id {
                let visibility: String = row
                    .try_get("visibility")
                    .map_err(|_| AuthorError::Unavailable)?;
                let moderation: String = row
                    .try_get("visibility_status")
                    .map_err(|_| AuthorError::Unavailable)?;
                target_ok = visibility == "public" && moderation == "active" && !held && !deleted;
            }
        }
        // Uniform 404 avoids probing unpublished targets or private owners.
        if !source_ok || !target_ok {
            return Err(AuthorError::Missing);
        }
        lock_manageable_member(&mut tx, actor, book_id).await?;
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
            "SELECT b.id FROM books b WHERE b.id=$1
               AND NOT b.admin_hold AND NOT b.is_deleted
             FOR UPDATE OF b",
        )
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if allowed.is_none() {
            return Err(AuthorError::Missing);
        }
        lock_manageable_member(&mut tx, actor, book_id).await?;
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
/// Explicit NEwBOO related-book links, not automatically inferred recommendations.
/// Both the source and every target must still pass the live public-book gate.
/// One indexed query, bounded to 12 cards; a hidden target never leaks a title.
impl PgCatalog {
    pub async fn public_related_books(
        &self,
        book_id: i64,
    ) -> Result<Vec<RelatedBookCard>, AuthorError> {
        if book_id <= 0 {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "SELECT target.id, target.title
             FROM books AS source
             JOIN book_related_links AS link
               ON source.id IN (link.smaller_book_id, link.larger_book_id)
             JOIN books AS target
               ON target.id = CASE WHEN source.id=link.smaller_book_id
                 THEN link.larger_book_id ELSE link.smaller_book_id END
             WHERE source.id=$1
               AND source.visibility='public' AND source.visibility_status='active'
               AND NOT source.admin_hold AND NOT source.is_deleted
               AND target.visibility='public' AND target.visibility_status='active'
               AND NOT target.admin_hold AND NOT target.is_deleted
             ORDER BY target.id LIMIT 12",
        )
        .bind(book_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        rows.into_iter()
            .map(|row| {
                Ok(RelatedBookCard {
                    id: row.try_get("id").map_err(|_| AuthorError::Unavailable)?,
                    title: row.try_get("title").map_err(|_| AuthorError::Unavailable)?,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn s020_self_link_does_not_create_an_edge() {
        assert_eq!(normalized_title(" Ёж — и   кот "), "еж и кот");
    }
}
