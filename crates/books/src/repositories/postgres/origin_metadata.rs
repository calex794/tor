//! S020: active Book member origin metadata edits, independent of publication and access.
use super::PgAuthoring;
use crate::services::authoring::{normalize_discovery_fields, AuthorError, DiscoveryFields};
use sqlx::Row;

pub struct ManagedOriginMetadata {
    pub title: String,
    pub original_language: String,
    pub release_year: Option<i32>,
    pub catalog_section: String,
    pub author_name: String,
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    pub fandoms: Vec<String>,
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
    pub async fn manageable_origin_metadata(
        &self,
        actor: i64,
        book_id: i64,
    ) -> Result<Option<ManagedOriginMetadata>, AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT b.title, b.original_language, b.release_year,
                    b.catalog_section, b.origin_metadata_revision,
                    b.author_name,b.genres,b.tags,b.fandoms
             FROM books b WHERE b.id=$1 AND NOT b.is_deleted AND NOT b.admin_hold
               AND b.lifecycle_status NOT IN ('pending_deletion','ordinary_deleted')
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
        row.map(|row| {
            Ok(ManagedOriginMetadata {
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
                author_name: row
                    .try_get("author_name")
                    .map_err(|_| AuthorError::Unavailable)?,
                genres: row.try_get("genres").map_err(|_| AuthorError::Unavailable)?,
                tags: row.try_get("tags").map_err(|_| AuthorError::Unavailable)?,
                fandoms: row.try_get("fandoms").map_err(|_| AuthorError::Unavailable)?,
                revision: row
                    .try_get("origin_metadata_revision")
                    .map_err(|_| AuthorError::Unavailable)?,
            })
        })
        .transpose()
    }

    /// Existing origin-only callers keep the original form contract.
    pub async fn update_origin_metadata(
        &self,
        actor: i64,
        book_id: i64,
        expected_revision: i64,
        raw_language: &str,
        raw_year: &str,
        raw_section: &str,
    ) -> Result<i64, AuthorError> {
        self.update_origin_metadata_inner(
            actor, book_id, expected_revision, raw_language, raw_year, raw_section, None,
        )
        .await
    }

    /// Full S020 settings submission changes origin and discovery labels together.
    /// No change to age, publication, source identity or existing purchases.
    pub async fn update_origin_and_discovery(
        &self,
        actor: i64,
        book_id: i64,
        expected_revision: i64,
        raw_language: &str,
        raw_year: &str,
        raw_section: &str,
        discovery: DiscoveryFields<'_>,
    ) -> Result<i64, AuthorError> {
        self.update_origin_metadata_inner(
            actor, book_id, expected_revision, raw_language, raw_year, raw_section,
            Some(discovery),
        )
        .await
    }

    async fn update_origin_metadata_inner(
        &self,
        actor: i64,
        book_id: i64,
        expected_revision: i64,
        raw_language: &str,
        raw_year: &str,
        raw_section: &str,
        discovery: Option<DiscoveryFields<'_>>,
    ) -> Result<i64, AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Err(AuthorError::Missing);
        }
        if expected_revision <= 0 {
            return Err(AuthorError::Invalid);
        }
        let (language, year, section) =
            validate_origin_metadata(raw_language, raw_year, raw_section)?;
        let discovery = discovery.map(normalize_discovery_fields).transpose()?;
        let author_name = discovery.as_ref().map(|data| data.author_name.clone());
        let genres = discovery.as_ref().map(|data| data.genres.clone());
        let tags = discovery.as_ref().map(|data| data.tags.clone());
        let fandoms = discovery.as_ref().map(|data| data.fandoms.clone());

        let mut tx = self.pool.begin().await.map_err(|_| AuthorError::Unavailable)?;
        // Serialize with Book/member revocation and the old origin-only editor.
        let revision: Option<i64> = sqlx::query_scalar(
            "SELECT b.origin_metadata_revision FROM books b
             WHERE b.id=$1 AND NOT b.is_deleted AND NOT b.admin_hold
               AND b.lifecycle_status NOT IN ('pending_deletion','ordinary_deleted')
             FOR UPDATE OF b",
        )
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let revision = revision.ok_or(AuthorError::Missing)?;
        super::book_related::lock_manageable_member(&mut tx, actor, book_id).await?;
        if revision != expected_revision {
            return Err(AuthorError::Conflict);
        }
        let next_revision = revision.checked_add(1).ok_or(AuthorError::Conflict)?;
        let updated: Option<i64> = sqlx::query_scalar(
            "UPDATE books
             SET original_language=$2, release_year=$3, catalog_section=$4,
                 origin_metadata_revision=$5,
                 author_name=COALESCE($7::TEXT, author_name),
                 genres=COALESCE($8::TEXT[], genres),
                 tags=COALESCE($9::TEXT[], tags),
                 fandoms=COALESCE($10::TEXT[], fandoms)
             WHERE id=$1 AND origin_metadata_revision=$6
             RETURNING origin_metadata_revision",
        )
        .bind(book_id)
        .bind(&language)
        .bind(year)
        .bind(&section)
        .bind(next_revision)
        .bind(expected_revision)
        .bind(author_name)
        .bind(genres)
        .bind(tags)
        .bind(fandoms)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if updated != Some(next_revision) {
            return Err(AuthorError::Conflict);
        }
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(next_revision)
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
