//! S020: atomic owner-only editing of book titles and their duplicate claims.
//! Published editions and private drafts keep their source identity and reader rights.
use super::PgAuthoring;
use crate::services::{authoring::AuthorError, book_registration::normalized_title};
use sqlx::Row;

pub struct OwnedBookTitles {
    pub title: String,
    pub original_title: String,
    pub alternatives: Vec<String>,
    pub revision: i64,
}

pub fn checked_titles(
    title: &str,
    original_title: &str,
    alternatives: [&str; 3],
) -> Result<(String, String, Vec<String>, Vec<String>), AuthorError> {
    let title = title.trim();
    let original = original_title.trim();
    if title.is_empty()
        || title.chars().count() > 200
        || title.chars().any(char::is_control)
        || original.chars().count() > 200
        || original.chars().any(char::is_control)
    {
        return Err(AuthorError::Invalid);
    }
    let mut alts = Vec::new();
    for raw in alternatives {
        let clean = raw.trim();
        if clean.is_empty() {
            continue;
        }
        if clean.chars().count() > 200 || clean.chars().any(char::is_control) {
            return Err(AuthorError::Invalid);
        }
        if !alts.iter().any(|prior: &String| prior == clean) {
            alts.push(clean.to_owned());
        }
    }
    let mut claims = Vec::new();
    for value in std::iter::once(title)
        .chain(std::iter::once(original))
        .chain(alts.iter().map(String::as_str))
    {
        if value.is_empty() {
            continue;
        }
        let claim = normalized_title(value);
        if claim.is_empty() || claim.chars().count() > 200 {
            return Err(AuthorError::Invalid);
        }
        if !claims.contains(&claim) {
            claims.push(claim);
        }
    }
    Ok((title.to_owned(), original.to_owned(), alts, claims))
}

impl PgAuthoring {
    pub async fn owned_book_titles(
        &self,
        actor: i64,
        book_id: i64,
    ) -> Result<Option<OwnedBookTitles>, AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT title, original_title, alt_titles, title_metadata_revision
               FROM books WHERE id=$1 AND owner_user_id=$2
               AND NOT is_deleted AND NOT admin_hold",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        row.map(|row| {
            Ok(OwnedBookTitles {
                title: row.try_get("title").map_err(|_| AuthorError::Unavailable)?,
                original_title: row
                    .try_get("original_title")
                    .map_err(|_| AuthorError::Unavailable)?,
                alternatives: row
                    .try_get("alt_titles")
                    .map_err(|_| AuthorError::Unavailable)?,
                revision: row
                    .try_get("title_metadata_revision")
                    .map_err(|_| AuthorError::Unavailable)?,
            })
        })
        .transpose()
    }

    pub async fn update_book_titles(
        &self,
        actor: i64,
        book_id: i64,
        expected_revision: i64,
        title: &str,
        original: &str,
        alternatives: [&str; 3],
    ) -> Result<i64, AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Err(AuthorError::Missing);
        }
        if expected_revision <= 0 {
            return Err(AuthorError::Invalid);
        }
        let (title, original, alternatives, claims) =
            checked_titles(title, original, alternatives)?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        let book = sqlx::query(
            "SELECT target_language, title_metadata_revision
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
            .try_get("title_metadata_revision")
            .map_err(|_| AuthorError::Unavailable)?;
        if revision != expected_revision {
            return Err(AuthorError::Conflict);
        }

        // Registered aliases of other books, including private drafts, must
        // reject duplicate names without revealing the conflicting book.
        let registered: Option<i64> = sqlx::query_scalar(
            "SELECT book_id FROM book_registration_titles
             WHERE target_language=$1 AND book_id<>$2
               AND normalized_title=ANY($3::text[]) LIMIT 1",
        )
        .bind(&language)
        .bind(book_id)
        .bind(&claims)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if registered.is_some() {
            return Err(AuthorError::Conflict);
        }

        // Historical rows may have no alias claims. Check each original and
        // alternative name with the existing indexed normalization helpers.
        let legacy: Option<i64> = sqlx::query_scalar(
            "SELECT b.id FROM books AS b
             WHERE b.target_language=$1 AND b.id<>$2 AND NOT b.is_deleted
               AND NOT EXISTS (
                 SELECT 1 FROM book_registration_titles AS c WHERE c.book_id=b.id
               )
               AND (
                 book_registration_legacy_title_key(b.title)=ANY($3::text[])
                 OR book_registration_legacy_alias_keys(b.original_title,b.alt_titles)
                      && $3::text[]
               )
             LIMIT 1",
        )
        .bind(&language)
        .bind(book_id)
        .bind(&claims)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if legacy.is_some() {
            return Err(AuthorError::Conflict);
        }

        let new_revision = revision.checked_add(1).ok_or(AuthorError::Unavailable)?;
        sqlx::query(
            "UPDATE books SET title=$3, original_title=$4, alt_titles=$5,
             title_metadata_revision=$6 WHERE id=$1 AND owner_user_id=$2",
        )
        .bind(book_id)
        .bind(actor)
        .bind(&title)
        .bind(&original)
        .bind(&alternatives)
        .bind(new_revision)
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        sqlx::query("DELETE FROM book_registration_titles WHERE book_id=$1")
            .bind(book_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        for normalized in &claims {
            let result = sqlx::query(
                "INSERT INTO book_registration_titles(book_id,target_language,normalized_title)
                 VALUES($1,$2,$3)",
            )
            .bind(book_id)
            .bind(&language)
            .bind(normalized)
            .execute(&mut *tx)
            .await;
            match result {
                Ok(_) => (),
                Err(sqlx::Error::Database(ref db)) if db.code().as_deref() == Some("23505") => {
                    return Err(AuthorError::Conflict)
                }
                Err(_) => return Err(AuthorError::Unavailable),
            }
        }
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(new_revision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn s020_chinese_english_aliases_are_part_of_same_claim_set() {
        let (_, _, alts, claims) = checked_titles(
            "Путь бессмертного",
            "修仙之路",
            ["The Path to Immortality", "", ""],
        )
        .unwrap();
        assert_eq!(alts, ["The Path to Immortality"]);
        assert_eq!(
            claims,
            ["путь бессмертного", "修仙之路", "the path to immortality"]
        );
        assert!(checked_titles(" ", "", ["", "", ""]).is_err());
        assert!(checked_titles("Book", "", ["<escape>", "", ""]).is_ok());
        assert!(checked_titles("Book", "", ["\u{0000}", "", ""]).is_err());
    }
}
