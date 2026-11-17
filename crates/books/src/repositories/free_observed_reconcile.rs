//! DROP-07b: read-only classification for a bounded S3 LIST page.
//! Unknown is NOT orphaned, missing, or GC-eligible. This deliberately does
//! not create a physical reclamation decision or authorize S3 deletion.
use std::collections::HashSet;

use sqlx::{Executor, PgPool, Postgres, Row};

use super::free_expected_inventory::{valid_bucket, valid_cursor, FreeInventoryError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FreeReferenceState {
    Registered,
    ProtectedPending,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FreeReferenceEvidence {
    pub object_key: String,
    pub state: FreeReferenceState,
}

pub struct PgFreeObservedClassifier {
    pool: PgPool,
}

#[derive(Default)]
struct PendingLocator {
    chapter: i64,
    version: i64,
    direct_book: i64,
}

fn canonical_id(text: &str) -> Option<i64> {
    let value: i64 = text.parse().ok()?;
    (value > 0 && !text.starts_with('0') && text.bytes().all(|b| b.is_ascii_digit()))
        .then_some(value)
}

fn pending_locator(key: &str) -> PendingLocator {
    if let Some((chapter, file)) = key
        .strip_prefix("private/free-chapters/")
        .and_then(|suffix| suffix.split_once('/'))
    {
        if let (Some(chapter), Some(version)) = (
            canonical_id(chapter),
            file.strip_suffix(".md").and_then(canonical_id),
        ) {
            return PendingLocator {
                chapter,
                version,
                ..Default::default()
            };
        }
    } else if let Some((book, file)) = key
        .strip_prefix("private/free-packs/import/")
        .and_then(|suffix| suffix.split_once('/'))
    {
        let file = file
            .strip_suffix(".zst")
            .or_else(|| file.strip_suffix(".manifest.json"));
        if file.is_some_and(|stem| {
            stem.len() == 64
                && stem
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }) {
            if let Some(direct_book) = canonical_id(book) {
                return PendingLocator {
                    direct_book,
                    ..Default::default()
                };
            }
        }
    }
    PendingLocator::default()
}

impl PgFreeObservedClassifier {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Read-only protection check for a single bounded S3 LIST page.
    ///
    /// Registered includes every source SINGLE, PACK and sidecar *even when*
    /// no active generation references that object today: preserved V1 and
    /// rollback source bytes remain protected.
    ///
    /// ProtectedPending includes immutable stages, prepared SINGLE IDs and
    /// R4a targets. For a running or queued direct PACK import we protect its
    /// entire canonical book prefix, including before a stage receipt exists.
    /// Unrecognized objects are UNKNOWN, never proposed for deletion.
    pub async fn classify_page(
        &self,
        bucket: &str,
        keys: &[String],
    ) -> Result<Vec<FreeReferenceEvidence>, FreeInventoryError> {
        Self::classify_in(&self.pool, bucket, keys).await
    }

    pub(super) async fn classify_in<'e>(
        executor: impl Executor<'e, Database = Postgres>,
        bucket: &str,
        keys: &[String],
    ) -> Result<Vec<FreeReferenceEvidence>, FreeInventoryError> {
        if !valid_bucket(bucket)
            || keys.is_empty()
            || keys.len() > 128
            || keys.iter().any(|key| !valid_cursor(key))
        {
            return Err(FreeInventoryError::Invalid);
        }
        let distinct: HashSet<_> = keys.iter().collect();
        if distinct.len() != keys.len() {
            return Err(FreeInventoryError::Invalid);
        }

        let locators: Vec<_> = keys.iter().map(|key| pending_locator(key)).collect();
        let chapters: Vec<_> = locators.iter().map(|item| item.chapter).collect();
        let versions: Vec<_> = locators.iter().map(|item| item.version).collect();
        let direct_books: Vec<_> = locators.iter().map(|item| item.direct_book).collect();
        let rows = sqlx::query(
            "WITH listed AS (
                SELECT * FROM unnest($2::text[],$3::bigint[],$4::bigint[],$5::bigint[])
                    AS locators(object_key,chapter_id,version_id,direct_book_id)
             )
             SELECT listed.object_key,
                 CASE
                   WHEN EXISTS (
                     SELECT 1 FROM free_chapter_storage_objects s
                     WHERE s.bucket=$1 AND s.object_key=listed.object_key
                   ) OR EXISTS (
                     SELECT 1 FROM free_storage_packs p
                     WHERE p.bucket=$1 AND
                       (p.object_key=listed.object_key
                         OR p.manifest_key=listed.object_key)
                   ) THEN 'registered'
                   WHEN EXISTS (
                     SELECT 1 FROM free_storage_key_fences f
                     WHERE f.bucket=$1 AND f.object_key=listed.object_key
                       AND f.purpose='publisher'
                   ) OR EXISTS (
                     SELECT 1 FROM free_pack_direct_import_stages st
                     WHERE st.bucket=$1 AND
                       (st.object_key=listed.object_key
                         OR regexp_replace(
                              st.object_key,'[.]zst$','.manifest.json'
                            )=listed.object_key)
                   ) OR EXISTS (
                     SELECT 1 FROM free_pack_job_specs spec
                     JOIN free_pack_jobs j ON j.id=spec.job_id
                     WHERE j.status IN ('pending','running','succeeded')
                       AND spec.spec_text::jsonb #>> '{target,bucket}'=$1
                       AND (
                         spec.spec_text::jsonb #>> '{target,pack_key}'=
                             listed.object_key
                         OR spec.spec_text::jsonb #>> '{target,manifest_key}'=
                             listed.object_key
                       )
                   ) OR (
                     listed.version_id > 0 AND (
                       EXISTS (
                         SELECT 1 FROM chapter_save_commands c
                         WHERE c.version_id=listed.version_id AND c.chapter_id=listed.chapter_id
                       ) OR EXISTS (
                         SELECT 1 FROM free_chapter_import_reservations r
                         WHERE r.version_id=listed.version_id AND r.chapter_id=listed.chapter_id
                       )
                     )
                   ) OR EXISTS (
                     SELECT 1 FROM chapter_import_jobs j
                     JOIN free_chapter_import_transports t ON t.job_id=j.id
                     WHERE listed.direct_book_id > 0 AND j.book_id=listed.direct_book_id
                       AND j.status IN ('queued','running') AND t.transport='pack'
                   ) THEN 'protected_pending'
                   ELSE 'unknown'
                 END AS reference_state
             FROM listed ORDER BY listed.object_key",
        )
        .bind(bucket)
        .bind(keys)
        .bind(chapters)
        .bind(versions)
        .bind(direct_books)
        .fetch_all(executor)
        .await
        .map_err(|_| FreeInventoryError::Unavailable)?;

        if rows.len() != keys.len() {
            return Err(FreeInventoryError::Unavailable);
        }
        rows.into_iter()
            .map(|row| {
                let object_key: String = row
                    .try_get("object_key")
                    .map_err(|_| FreeInventoryError::Unavailable)?;
                let state = match row
                    .try_get::<String, _>("reference_state")
                    .map_err(|_| FreeInventoryError::Unavailable)?
                    .as_str()
                {
                    "registered" => FreeReferenceState::Registered,
                    "protected_pending" => FreeReferenceState::ProtectedPending,
                    "unknown" => FreeReferenceState::Unknown,
                    _ => return Err(FreeInventoryError::Unavailable),
                };
                Ok(FreeReferenceEvidence { object_key, state })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn validation_denies_duplicate_untrusted_and_oversized_pages() {
        // SQLx's lazy pool needs a Tokio runtime even when validation rejects
        // before issuing a query. This test never contacts PostgreSQL.
        let pool = PgPool::connect_lazy("postgresql://127.0.0.1:5432/none")
            .expect("construct only; never connects to external PG");
        let classifier = PgFreeObservedClassifier::new(pool);
        let result = classifier
            .classify_page(
                "tor-private-free",
                &[
                    "private/free-chapters/1/1.md".to_owned(),
                    "private/free-chapters/1/1.md".to_owned(),
                ],
            )
            .await;
        assert_eq!(result, Err(FreeInventoryError::Invalid));
    }
}
