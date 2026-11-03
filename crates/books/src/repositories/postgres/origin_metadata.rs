//! S020: owner-only origin metadata edits, independent of publication and access.
use super::PgAuthoring;
use crate::services::authoring::AuthorError;
use sqlx::Row;

pub struct OwnedOriginMetadata {
    pub title: String,
    pub original_language: String,
    pub release_year: Option<i32>,
    pub catalog_section: String,
    pub revision: i64,
}

pub fn validate_origin_metadata(
    raw_language: &str,
    raw_year: &str,
    raw_section: &str,
) -> Result<(String, Option<i32>, String), AuthorError> {
    let language = raw_language.trim().to_ascii_lowercase();
    let section = raw_section.trim();
    if language.len() > 32
        || (!language.is_empty()
            && !language
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'-'))
        || (!section.is_empty()
            && !matches!(
                section,
                "novels/chinese"
                    | "novels/korean"
                    | "novels/japanese"
                    | "novels/english"
                    | "novels/original"
                    | "fanfic/translated"
            ))
    {
        return Err(AuthorError::Invalid);
    }
    let year = if raw_year.trim().is_empty() {
        None
    } else {
        let value = raw_year.trim();
        if value.len() != 4 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(AuthorError::Invalid);
        }
        let year: i32 = value.parse().map_err(|_| AuthorError::Invalid)?;
        if !(1000..=9999).contains(&year) {
            return Err(AuthorError::Invalid);
        }
        Some(year)
    };
    Ok((language, year, section.to_owned()))
}

impl PgAuthoring {
    pub async fn owned_origin_metadata(
        &self,
        actor: i64,
        book_id: i64,
    ) -> Result<Option<OwnedOriginMetadata>, AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT title, original_language, release_year, catalog_section,
                    origin_metadata_revision
             FROM books
             WHERE id=$1 AND owner_user_id=$2 AND NOT is_deleted AND NOT admin_hold",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        row.map(|row| {
            Ok(OwnedOriginMetadata {
                title: row.try_get("title").map_err(|_| AuthorError::Unavailable)?,
                original_language: row
                    .try_get("original_language")
                    .map_err(|_| AuthorError::Unavailable)?,
                release_year: row
                    .try_get("release_year")
                    .map_err(|_| AuthorError::Unavailable)?,
                catalog_section: row
                    .try_get("catalog_section")
                    .map_err(|_| AuthorError::Unavailable)?,
                revision: row
                    .try_get("origin_metadata_revision")
                    .map_err(|_| AuthorError::Unavailable)?,
            })
        })
        .transpose()
    }

    pub async fn update_origin_metadata(
        &self,
        actor: i64,
        book_id: i64,
        expected_revision: i64,
        raw_language: &str,
        raw_year: &str,
        raw_section: &str,
    ) -> Result<i64, AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Err(AuthorError::Missing);
        }
        if expected_revision <= 0 {
            return Err(AuthorError::Invalid);
        }
        let (language, year, section) =
            validate_origin_metadata(raw_language, raw_year, raw_section)?;
        let updated: Option<i64> = sqlx::query_scalar(
            "UPDATE books
                SET original_language=$4, release_year=$5, catalog_section=$6,
                    origin_metadata_revision=origin_metadata_revision+1
              WHERE id=$1 AND owner_user_id=$2
                AND origin_metadata_revision=$3
                AND origin_metadata_revision < 9223372036854775807
                AND NOT is_deleted AND NOT admin_hold
              RETURNING origin_metadata_revision",
        )
        .bind(book_id)
        .bind(actor)
        .bind(expected_revision)
        .bind(&language)
        .bind(year)
        .bind(&section)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if let Some(revision) = updated {
            return Ok(revision);
        }
        if self.owned_origin_metadata(actor, book_id).await?.is_some() {
            Err(AuthorError::Conflict)
        } else {
            Err(AuthorError::Missing)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s020_origin_validation_matches_registration_contract() {
        assert_eq!(
            validate_origin_metadata(" ZH ", "2024", "novels/chinese").unwrap(),
            ("zh".to_owned(), Some(2024), "novels/chinese".to_owned())
        );
        assert_eq!(
            validate_origin_metadata("", "", "").unwrap(),
            ("".to_owned(), None, "".to_owned())
        );
        for year in ["0999", "10000", "abcd", "2024.5", "2025\n"] {
            if year == "2025\n" {
                assert_eq!(
                    validate_origin_metadata("en", year, "").unwrap().1,
                    Some(2025)
                );
            } else {
                assert!(validate_origin_metadata("en", year, "").is_err());
            }
        }
        for language in ["ru;DROP", "русский", "z_h"] {
            assert!(validate_origin_metadata(language, "", "").is_err());
        }
        assert!(validate_origin_metadata("zh", "", "paid").is_err());
    }
}
