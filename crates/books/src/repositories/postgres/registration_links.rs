//! Claim all known links for the original book without guessing equivalence
//! between unrelated hosts. No external page loads or private-book disclosure.
use super::source_alias_guard;
use crate::services::{
    authoring::{AuthorError, NewBook},
    book_registration::{registration_link_claims, rulate_work_id},
};
use sqlx::{Postgres, Transaction};

pub(super) type LinkClaims = Vec<(String, String)>;

pub(super) async fn check(
    tx: &mut Transaction<'_, Postgres>,
    book: &NewBook,
) -> Result<LinkClaims, AuthorError> {
    let links = registration_link_claims(book)?;
    if links.is_empty() {
        return Ok(links);
    }
    let identities: Vec<&str> = links.iter().map(|(id, _)| id.as_str()).collect();
    let urls: Vec<&str> = links.iter().map(|(_, url)| url.as_str()).collect();

    let claimed: Option<i64> = sqlx::query_scalar(
        "SELECT book_id FROM book_registration_links
         WHERE target_language=$1 AND link_identity = ANY($2::text[]) LIMIT 1",
    )
    .bind(&book.target_language)
    .bind(&identities)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    if claimed.is_some() {
        return Err(AuthorError::Conflict);
    }

    // Check preexisting books, including imports before source-link claims.
    let historical: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM books
         WHERE target_language=$1 AND NOT is_deleted
           AND (source_identity=ANY($2::text[]) OR source_url=ANY($3::text[]))
         LIMIT 1",
    )
    .bind(&book.target_language)
    .bind(&identities)
    .bind(&urls)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    if historical.is_some() {
        return Err(AuthorError::Conflict);
    }

    // A historical Rulate link may contain a different scheme/query/host.
    for (_, url) in &links {
        if let Some(work_id) = rulate_work_id(url) {
            if source_alias_guard::rulate_work_exists(
                tx,
                &book.target_language,
                &work_id,
            )
            .await?
            {
                return Err(AuthorError::Conflict);
            }
        }
    }
    Ok(links)
}

pub(super) async fn claim(
    tx: &mut Transaction<'_, Postgres>,
    book_id: i64,
    language: &str,
    links: &LinkClaims,
) -> Result<(), AuthorError> {
    for (identity, url) in links {
        let inserted = sqlx::query(
            "INSERT INTO book_registration_links
             (book_id, target_language, link_identity, source_url)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(book_id)
        .bind(language)
        .bind(identity)
        .bind(url)
        .execute(&mut **tx)
        .await;
        match inserted {
            Ok(_) => (),
            Err(sqlx::Error::Database(ref db)) if db.code().as_deref() == Some("23505") => {
                return Err(AuthorError::Conflict);
            }
            Err(_) => return Err(AuthorError::Unavailable),
        }
    }
    Ok(())
}
