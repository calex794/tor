//! Claim all known links for the original book without guessing equivalence
//! between unrelated hosts. No external page loads or private-book disclosure.
use super::source_alias_guard;
use crate::services::{
    authoring::{AuthorError, NewBook},
    book_registration::{normalized_source_url, registration_link_claims},
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

    // Legacy primary URLs and old additional links use the same canonical
    // comparison as new claims, regardless of source website.
    let canonical_urls = links
        .iter()
        .map(|(_, url)| normalized_source_url(url))
        .collect::<Result<Vec<_>, _>>()?;
    if source_alias_guard::source_url_exists(tx, &book.target_language, &canonical_urls, None)
        .await?
    {
        return Err(AuthorError::Conflict);
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

/// Only the actual Book owner can read the immutable registration outcome.
/// Book visibility/hold are independent operational states, not review results.
/// Reviewer identity, decision timestamp and internal identifiers stay private.
impl super::PgAuthoring {
    pub async fn owner_registration_status(
        &self,
        actor: i64,
        book_id: i64,
    ) -> Result<Option<(String, Option<String>, String, bool)>, AuthorError> {
        use sqlx::Row;
        if actor <= 0 || book_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT CASE
                 WHEN b.registration_approval_decision_id IS NOT NULL THEN 'approved'
                 WHEN d.outcome IN ('approved','rejected') THEN d.outcome
                 WHEN b.registration_requires_review THEN 'pending'
                 ELSE 'not_required'
               END AS registration_outcome,
               CASE WHEN b.registration_approval_decision_id IS NULL
                         AND d.outcome='rejected' THEN d.reason ELSE NULL END
                   AS rejection_reason,
               b.visibility_status,b.admin_hold
             FROM books b
             LEFT JOIN LATERAL (
                 SELECT outcome,reason FROM book_registration_decisions
                 WHERE book_id=b.id ORDER BY id DESC LIMIT 1
             ) d ON TRUE
             WHERE b.id=$1 AND b.owner_user_id=$2 AND NOT b.is_deleted",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        row.map(|row| {
            Ok((
                row.try_get("registration_outcome")
                    .map_err(|_| AuthorError::Unavailable)?,
                row.try_get("rejection_reason")
                    .map_err(|_| AuthorError::Unavailable)?,
                row.try_get("visibility_status")
                    .map_err(|_| AuthorError::Unavailable)?,
                row.try_get("admin_hold")
                    .map_err(|_| AuthorError::Unavailable)?,
            ))
        })
        .transpose()
    }
}
