//! Owner-only source-link editor. A link is an attribution, not a book ID.
//! Historical source_identity is immutable; chapter, purchase and access rights
//! never change when the owner updates these URLs.
use super::PgAuthoring;
use crate::services::{
    authoring::AuthorError,
    book_registration::{edited_link_claims, rulate_work_id},
};
use sqlx::Row;

pub struct OwnedBookSourceLinks {
    pub title: String,
    pub primary: String,
    pub extra: Vec<String>,
    pub revision: i64,
}

impl PgAuthoring {
    /// Do not reveal even the existence of an unpublished book to non-owners.
    pub async fn owned_book_source_links(
        &self,
        actor: i64,
        book_id: i64,
    ) -> Result<Option<OwnedBookSourceLinks>, AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT title, source_url, registration_links_revision
             FROM books WHERE id=$1 AND owner_user_id=$2
             AND NOT is_deleted AND NOT admin_hold",
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
        Ok(Some(OwnedBookSourceLinks {
            title: row.try_get("title").map_err(|_| AuthorError::Unavailable)?,
            primary,
            extra,
            revision: row
                .try_get("registration_links_revision")
                .map_err(|_| AuthorError::Unavailable)?,
        }))
    }

    /// All checks, ownership and changes are in the same transaction. A stale
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
            "SELECT target_language, registration_links_revision
             FROM books WHERE id=$1 AND owner_user_id=$2
             AND NOT is_deleted AND NOT admin_hold FOR UPDATE",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?
        .ok_or(AuthorError::Missing)?;

        let language: String = book
            .try_get("target_language")
            .map_err(|_| AuthorError::Unavailable)?;
        let revision: i64 = book
            .try_get("registration_links_revision")
            .map_err(|_| AuthorError::Unavailable)?;
        if revision != expected_revision {
            return Err(AuthorError::Conflict);
        }
        // Check URL validity only after proving ownership of the row.
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

            let rulate_ids: Vec<String> =
                urls.iter().filter_map(|url| rulate_work_id(url)).collect();
            if !rulate_ids.is_empty() {
                let matches: Option<i64> = sqlx::query_scalar(
                    "SELECT id FROM books WHERE target_language=$1 AND id<>$2
                     AND NOT is_deleted AND source_url<>''
                     AND book_registration_rulate_work_id(source_url)=ANY($3::text[])
                     LIMIT 1",
                )
                .bind(&language)
                .bind(book_id)
                .bind(&rulate_ids)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|_| AuthorError::Unavailable)?;
                if matches.is_some() {
                    return Err(AuthorError::Conflict);
                }
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
             WHERE id=$3 AND owner_user_id=$4",
        )
        .bind(raw_links[0].trim())
        .bind(new_revision)
        .bind(book_id)
        .bind(actor)
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(new_revision)
    }
}
