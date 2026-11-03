//! Active Book member source-link editor. A link is attribution, not a book ID.
//! Historical source_identity is immutable; chapter, purchase and access rights
//! never change when an active Book member updates these URLs.
use super::PgAuthoring;
use crate::services::{
    authoring::AuthorError,
    book_registration::{edited_link_claims, normalized_source_url},
};
use sqlx::Row;

pub struct ManagedBookSourceLinks {
    pub title: String,
    pub primary: String,
    pub extra: Vec<String>,
    pub revision: i64,
}

impl PgAuthoring {
    /// Do not reveal an unpublished book to actors outside its active Book membership.
    pub async fn manageable_book_source_links(
        &self,
        actor: i64,
        book_id: i64,
    ) -> Result<Option<ManagedBookSourceLinks>, AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT b.title, b.source_url, b.registration_links_revision
             FROM books b WHERE b.id=$1 AND NOT b.is_deleted AND NOT b.admin_hold
               AND EXISTS (
                 SELECT 1 FROM book_members m WHERE m.book_id=b.id
                   AND m.user_id=$2 AND m.is_active
                   AND (m.role IN ('translator','moderator')
                        OR (m.role='owner' AND b.owner_user_id=$2))
               )",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let primary: String = row
            .try_get("source_url")
            .map_err(|_| AuthorError::Unavailable)?;
        let extra = sqlx::query_scalar::<_, String>(
            "SELECT source_url FROM book_registration_links
             WHERE book_id=$1 AND source_url<>$2
             ORDER BY source_url LIMIT 3",
        )
        .bind(book_id)
        .bind(&primary)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if extra.len() > 2 {
            return Err(AuthorError::Unavailable);
        }
        Ok(Some(ManagedBookSourceLinks {
            title: row.try_get("title").map_err(|_| AuthorError::Unavailable)?,
            primary,
            extra,
            revision: row
                .try_get("registration_links_revision")
                .map_err(|_| AuthorError::Unavailable)?,
        }))
    }

    /// Member check, source claims and changes share one transaction. A stale
    /// form receives a conflict instead of overwriting another tab's changes.
    pub async fn update_book_source_links(
        &self,
        actor: i64,
        book_id: i64,
        expected_revision: i64,
        raw_links: [&str; 3],
    ) -> Result<i64, AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Err(AuthorError::Missing);
        }
        if expected_revision <= 0 {
            return Err(AuthorError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        let book = sqlx::query(
            "SELECT b.target_language, b.registration_links_revision
             FROM books b WHERE b.id=$1
             AND NOT b.is_deleted AND NOT b.admin_hold FOR UPDATE OF b",
        )
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?
        .ok_or(AuthorError::Missing)?;
        super::book_related::lock_manageable_member(&mut tx, actor, book_id).await?;

        let language: String = book
            .try_get("target_language")
            .map_err(|_| AuthorError::Unavailable)?;
        let revision: i64 = book
            .try_get("registration_links_revision")
            .map_err(|_| AuthorError::Unavailable)?;
        if revision != expected_revision {
            return Err(AuthorError::Conflict);
        }
        // Validate URLs only after proving active membership of this Book.
        let claims = edited_link_claims(raw_links)?;
        let identities: Vec<String> = claims.iter().map(|(id, _)| id.clone()).collect();
        let urls: Vec<String> = claims.iter().map(|(_, url)| url.clone()).collect();

        if !claims.is_empty() {
            let claimed: Option<i64> = sqlx::query_scalar(
                "SELECT book_id FROM book_registration_links
                 WHERE target_language=$1 AND book_id<>$2
                   AND link_identity=ANY($3::text[]) LIMIT 1",
            )
            .bind(&language)
            .bind(book_id)
            .bind(&identities)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
            if claimed.is_some() {
                return Err(AuthorError::Conflict);
            }

            let historical: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM books WHERE target_language=$1 AND id<>$2
                 AND NOT is_deleted
                 AND (source_identity=ANY($3::text[]) OR source_url=ANY($4::text[]))
                 LIMIT 1",
            )
            .bind(&language)
            .bind(book_id)
            .bind(&identities)
            .bind(&urls)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
            if historical.is_some() {
                return Err(AuthorError::Conflict);
            }

            let canonical_urls = urls
                .iter()
                .map(|url| normalized_source_url(url))
                .collect::<Result<Vec<_>, _>>()?;
            if super::source_alias_guard::source_url_exists(
                &mut tx,
                &language,
                &canonical_urls,
                Some(book_id),
            )
            .await?
            {
                return Err(AuthorError::Conflict);
            }
        }

        // Replace only this book's claims; the UNIQUE constraint resolves
        // concurrent attempts to claim the same source URL across books.
        sqlx::query("DELETE FROM book_registration_links WHERE book_id=$1")
            .bind(book_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        for (identity, url) in &claims {
            let result = sqlx::query(
                "INSERT INTO book_registration_links
                 (book_id, target_language, link_identity, source_url)
                 VALUES($1,$2,$3,$4)",
            )
            .bind(book_id)
            .bind(&language)
            .bind(identity)
            .bind(url)
            .execute(&mut *tx)
            .await;
            match result {
                Ok(_) => (),
                Err(sqlx::Error::Database(ref db)) if db.code().as_deref() == Some("23505") => {
                    return Err(AuthorError::Conflict);
                }
                Err(_) => return Err(AuthorError::Unavailable),
            }
        }

        let new_revision = revision.checked_add(1).ok_or(AuthorError::Unavailable)?;
        sqlx::query(
            "UPDATE books SET source_url=$1, registration_links_revision=$2
             WHERE id=$3",
        )
        .bind(raw_links[0].trim())
        .bind(new_revision)
        .bind(book_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(new_revision)
    }
}
