//! S349: free S3 reference resolved under current publication, age and hold ACL.
//! A standalone object reference is never evidence of read permission.
use sqlx::{PgPool, Row};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FreeS3Ref {
    pub chapter_id: i64,
    pub version_id: i64,
    pub sha256: [u8; 32],
    pub size_bytes: u64,
    pub bucket: String,
}

#[derive(Clone)]
pub struct PgFreeS3Ref {
    pool: PgPool,
}

impl PgFreeS3Ref {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Authorization and exact published version selection share a single
    /// PostgreSQL statement. A saved/free or paid library item is not a grant.
    pub async fn for_session(
        &self,
        chapter_id: i64,
        session_digest: Option<&[u8; 32]>,
    ) -> Result<Option<FreeS3Ref>, sqlx::Error> {
        if chapter_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT f.chapter_id,f.version_id,f.sha256,f.size_bytes,f.bucket
             FROM chapters c
             JOIN books b ON b.id=c.book_id
             JOIN chapter_publications p ON p.chapter_id=c.id
             JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=p.version_id
             JOIN free_chapter_storage_objects f
               ON f.chapter_id=v.chapter_id AND f.version_id=v.id
             WHERE c.id=$1
               AND c.visibility='public' AND c.access_level='free'
               AND NOT c.admin_hold AND NOT c.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
               AND b.visibility='public' AND b.visibility_status='active'
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND (
                   b.access_level='basic' OR EXISTS(
                       SELECT 1 FROM sessions sess
                       JOIN users acct ON acct.id=sess.user_id
                       WHERE sess.token_hash=$2::bytea AND sess.revoked_at IS NULL
                         AND sess.expires_at>now()
                         AND acct.account_state='active'
                         AND (
                           (b.access_level='adult' AND acct.adult_enabled)
                           OR
                           (b.access_level='pro_adult' AND EXISTS(
                               SELECT 1 FROM user_level_grants ag
                               WHERE ag.user_id=acct.id AND ag.level='pro_adult'
                                 AND ag.is_active
                           ))
                         )
                   )
               )",
        )
        .bind(chapter_id)
        .bind(session_digest.map(|digest| digest.as_slice()))
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            let digest: Vec<u8> = row.try_get("sha256")?;
            let sha256: [u8; 32] = digest
                .try_into()
                .map_err(|_| sqlx::Error::Decode("invalid free S3 SHA256".into()))?;
            let size_bytes: i64 = row.try_get("size_bytes")?;
            let size_bytes = u64::try_from(size_bytes)
                .map_err(|_| sqlx::Error::Decode("invalid free S3 size".into()))?;
            Ok(FreeS3Ref {
                chapter_id: row.try_get("chapter_id")?,
                version_id: row.try_get("version_id")?,
                sha256,
                size_bytes,
                bucket: row.try_get("bucket")?,
            })
        })
        .transpose()
    }
}
