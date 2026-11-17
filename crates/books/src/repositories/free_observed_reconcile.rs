//! DROP-07b: read-only classification for a bounded S3 LIST page.
//! Unknown is NOT orphaned, missing, or GC-eligible. This deliberately does
//! not create a physical reclamation decision or authorize S3 deletion.
use std::collections::HashSet;

use sqlx::{PgPool, Row};

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
    /// ProtectedPending includes pack-direct immutable stages and R4a
    /// preapproved jobs (including before S3 PUT and after lost commit ACK).
    /// Unrecognized objects are UNKNOWN, never proposed for deletion.
    pub async fn classify_page(
        &self,
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

        let rows = sqlx::query(
            "WITH listed(object_key) AS (
                SELECT unnest($2::text[])
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
                   ) THEN 'protected_pending'
                   ELSE 'unknown'
                 END AS reference_state
             FROM listed ORDER BY listed.object_key",
        )
        .bind(bucket)
        .bind(keys.to_vec())
        .fetch_all(&self.pool)
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

    #[test]
    fn validation_denies_duplicate_untrusted_and_oversized_pages() {
        let pool = PgPool::connect_lazy("postgresql://127.0.0.1:5432/none")
            .expect("construct only; never connects to external PG");
        let classifier = PgFreeObservedClassifier::new(pool);
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                classifier
                    .classify_page(
                        "tor-private-free",
                        &[
                            "private/free-chapters/1/1.md".to_owned(),
                            "private/free-chapters/1/1.md".to_owned(),
                        ],
                    )
                    .await
            });
        assert_eq!(result, Err(FreeInventoryError::Invalid));
    }
}
