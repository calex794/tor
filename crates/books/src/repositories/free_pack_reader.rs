//! DROP-R5a: B-only read-only lookup of an independently verified private
//! PACK candidate. C must authorize the exact edition before calling this;
//! physical placement metadata is never a reader grant or public S3 URL.
use super::free_pack_attach::PgFreePackCatalog;
use crate::services::authoring::AuthorError;
use sqlx::Row;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedFreePackCandidate {
    pub bucket: String,
    pub key: String,
    pub pack_bytes: u64,
    pub seek_table_start: u64,
    pub frame_offset: u64,
    pub frame_bytes: u64,
    pub pack_sha256: [u8; 32],
    pub manifest_sha256: [u8; 32],
}

impl PgFreePackCatalog {
    /// A candidate is usable for *internal* preflight only after R4a
    /// successfully commits the exact verified PACK in a succeeded job.
    /// This lookup deliberately ignores the active storage pointer: neither
    /// it nor the caller is allowed to switch live S349 reading.
    pub async fn verified_free_range_candidate(
        &self,
        chapter: i64,
        version: i64,
        sha256: [u8; 32],
        size_bytes: u64,
        original_bucket: &str,
    ) -> Result<Option<VerifiedFreePackCandidate>, AuthorError> {
        if chapter <= 0
            || version <= 0
            || !(1..=200_000).contains(&size_bytes)
            || sha256 == [0; 32]
            || original_bucket.is_empty()
        {
            return Err(AuthorError::Invalid);
        }
        let row = sqlx::query(
            "SELECT p.bucket,p.object_key,p.pack_bytes,p.seek_table_start,
                    g.frame_offset,g.frame_bytes,
                    p.pack_sha256,p.manifest_sha256
             FROM free_edition_storage_generations g
             JOIN free_storage_packs p ON p.id=g.pack_id
             JOIN free_pack_jobs j ON j.pack_id=p.id AND j.status='succeeded'
             JOIN chapter_versions v
               ON v.chapter_id=g.chapter_id AND v.id=g.version_id
             WHERE g.chapter_id=$1 AND g.version_id=$2
               AND g.placement_kind='pack' AND g.content_sha256=$3
               AND g.size_bytes=$4 AND v.content_sha256=$3
               AND p.bucket=$5
             ORDER BY g.generation DESC LIMIT 1",
        )
        .bind(chapter)
        .bind(version)
        .bind(sha256.as_slice())
        .bind(size_bytes as i64)
        .bind(original_bucket)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let signed = |name: &str| -> Result<u64, AuthorError> {
            let value: i64 = row.try_get(name).map_err(|_| AuthorError::Unavailable)?;
            u64::try_from(value).map_err(|_| AuthorError::Conflict)
        };
        let digest = |name: &str| -> Result<[u8; 32], AuthorError> {
            let value: Vec<u8> = row.try_get(name).map_err(|_| AuthorError::Unavailable)?;
            value.try_into().map_err(|_| AuthorError::Conflict)
        };
        Ok(Some(VerifiedFreePackCandidate {
            bucket: row
                .try_get("bucket")
                .map_err(|_| AuthorError::Unavailable)?,
            key: row
                .try_get("object_key")
                .map_err(|_| AuthorError::Unavailable)?,
            pack_bytes: signed("pack_bytes")?,
            seek_table_start: signed("seek_table_start")?,
            frame_offset: signed("frame_offset")?,
            frame_bytes: signed("frame_bytes")?,
            pack_sha256: digest("pack_sha256")?,
            manifest_sha256: digest("manifest_sha256")?,
        }))
    }
}
