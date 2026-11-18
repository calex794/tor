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

    /// D-05: a read-only physical SINGLE identity for a frozen FREE edition.
    /// CAUTION: this is NOT a reader authorization. HTTP callers must first
    /// require D's actor-owned historic Layer term, then repeat both D and
    /// this exact storage lookup after signing the short-lived capability.
    /// Neither client-selected versions nor a withdrawn PACK may be served
    /// by falling back to a previous SINGLE receipt.
    pub async fn archived_free_single_metadata_only(
        &self,
        chapter: i64,
        version: i64,
    ) -> Result<Option<FreeS3Ref>, sqlx::Error> {
        if chapter <= 0 || version <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT f.chapter_id,f.version_id,f.sha256,f.size_bytes,f.bucket
             FROM free_chapter_storage_objects f
             JOIN chapter_versions v ON v.id=f.version_id
               AND v.chapter_id=f.chapter_id AND v.content_sha256=f.sha256
             WHERE f.chapter_id=$1 AND f.version_id=$2
               AND NOT EXISTS (
                    SELECT 1 FROM free_edition_active_storage a
                    JOIN free_edition_storage_generations g
                      ON g.chapter_id=a.chapter_id
                     AND g.version_id=a.version_id
                     AND g.generation=a.generation
                    WHERE a.chapter_id=f.chapter_id
                      AND a.version_id=f.version_id
                      AND g.placement_kind='pack'
               ) LIMIT 1",
        )
        .bind(chapter)
        .bind(version)
        .fetch_optional(&self.pool)
        .await?;
        row.map(decode).transpose()
    }

    /// D-05: storage identity only, never a reader grant. The frozen edition
    /// comes from the actor-scoped historical Layer term, not a browser input.
    /// Return only a whole-object-verified and activated PACK generation.
    pub async fn archived_free_active_pack_metadata_only(
        &self,
        chapter: i64,
        version: i64,
    ) -> Result<Option<FreeS3Ref>, sqlx::Error> {
        if chapter <= 0 || version <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT g.chapter_id,g.version_id,g.content_sha256 AS sha256,
                    g.size_bytes,pack.bucket
             FROM free_edition_active_storage a
             JOIN free_edition_storage_generations g
               ON g.chapter_id=a.chapter_id AND g.version_id=a.version_id
              AND g.generation=a.generation AND g.placement_kind='pack'
             JOIN chapter_versions v
               ON v.chapter_id=g.chapter_id AND v.id=g.version_id
              AND v.content_sha256=g.content_sha256
             JOIN free_storage_packs pack ON pack.id=g.pack_id
             WHERE a.chapter_id=$1 AND a.version_id=$2
               AND g.size_bytes BETWEEN 1 AND 200000
             LIMIT 1",
        )
        .bind(chapter)
        .bind(version)
        .fetch_optional(&self.pool)
        .await?;
        row.map(decode).transpose()
    }

    /// Exact historical free file for an independently authenticated site
    /// administrator. A moderator or browser-selected role is insufficient.
    pub async fn for_administrator(
        &self,
        chapter: i64,
        version: i64,
        digest: &[u8; 32],
    ) -> Result<Option<FreeS3Ref>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT f.chapter_id,f.version_id,f.sha256,f.size_bytes,f.bucket
            FROM free_chapter_storage_objects f
            JOIN chapter_versions v ON v.chapter_id=f.chapter_id AND v.id=f.version_id
              AND v.content_sha256=f.sha256
            JOIN chapters c ON c.id=v.chapter_id
            JOIN users u ON u.account_state='active'
            JOIN sessions s ON s.user_id=u.id AND s.token_hash=$3
              AND s.revoked_at IS NULL AND s.expires_at>now()
            JOIN account_roles r ON r.user_id=u.id AND r.role='administrator' AND r.is_active
            WHERE f.chapter_id=$1 AND f.version_id=$2 LIMIT 1",
        )
        .bind(chapter)
        .bind(version)
        .bind(digest.as_slice())
        .fetch_optional(&self.pool)
        .await?;
        row.map(decode).transpose()
    }

    pub async fn for_saved_session(
        &self,
        chapter: i64,
        version: i64,
        digest: Option<&[u8; 32]>,
    ) -> Result<Option<FreeS3Ref>, sqlx::Error> {
        let Some(digest) = digest else {
            return Ok(None);
        };
        let row=sqlx::query("SELECT f.chapter_id,f.version_id,f.sha256,f.size_bytes,f.bucket
            FROM reader_saved_free_chapters pin
            JOIN chapters c ON c.id=pin.chapter_id
            JOIN books b ON b.id=c.book_id
            JOIN users u ON u.id=pin.user_id AND u.account_state='active'
            JOIN sessions sess ON sess.user_id=u.id AND sess.token_hash=$3::bytea
              AND sess.revoked_at IS NULL AND sess.expires_at>now()
            JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=pin.version_id
            JOIN free_chapter_storage_objects f ON f.chapter_id=c.id AND f.version_id=v.id
              AND f.sha256=v.content_sha256
            WHERE pin.chapter_id=$1 AND pin.version_id=$2 AND c.visibility='public'
              AND b.visibility_status='active' AND NOT b.admin_hold AND NOT b.is_deleted
              AND NOT c.admin_hold AND NOT c.is_deleted
              AND (b.visibility='public' OR b.lifecycle_status='pending_deletion'
                OR (b.lifecycle_status='ordinary_deleted' AND b.ordinary_deleted_at IS NOT NULL))
              AND (b.access_level='basic' OR (b.access_level='adult' AND u.adult_enabled)
                OR (b.access_level='pro_adult' AND EXISTS(
                    SELECT 1 FROM user_level_grants g WHERE g.user_id=u.id AND g.level='pro_adult' AND g.is_active)))")
            .bind(chapter).bind(version).bind(digest.as_slice())
            .fetch_optional(&self.pool).await?;
        row.map(decode).transpose()
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
        row.map(decode).transpose()
    }
}

/// DROP-06a: the *same C authorization* as S349, but for a bodyless FREE
/// edition whose active physical generation is an immutable verified PACK.
/// This path must never require a fictional SINGLE receipt, nor authorize a
/// candidate PACK that has not been made the exact active edition.
impl PgFreeS3Ref {
    pub async fn for_active_pack_session(
        &self,
        chapter_id: i64,
        session_digest: Option<&[u8; 32]>,
    ) -> Result<Option<FreeS3Ref>, sqlx::Error> {
        if chapter_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT g.chapter_id,g.version_id,g.content_sha256 AS sha256,
                    g.size_bytes,pack.bucket
             FROM chapters c
             JOIN books b ON b.id=c.book_id
             JOIN chapter_publications pub ON pub.chapter_id=c.id
             JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=pub.version_id
             JOIN free_edition_active_storage a
               ON a.chapter_id=v.chapter_id AND a.version_id=v.id
             JOIN free_edition_storage_generations g
               ON g.chapter_id=a.chapter_id AND g.version_id=a.version_id
              AND g.generation=a.generation AND g.placement_kind='pack'
              AND g.content_sha256=v.content_sha256
             JOIN free_storage_packs pack ON pack.id=g.pack_id
             WHERE c.id=$1 AND c.visibility='public'
               AND c.access_level='free' AND c.price_amount=0
               AND NOT c.admin_hold AND NOT c.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
               AND b.visibility='public' AND b.visibility_status='active'
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND (b.access_level='basic' OR EXISTS (
                   SELECT 1 FROM sessions sess
                   JOIN users acct ON acct.id=sess.user_id
                   WHERE sess.token_hash=$2::bytea AND sess.revoked_at IS NULL
                     AND sess.expires_at>now() AND acct.account_state='active'
                     AND (
                       (b.access_level='adult' AND acct.adult_enabled)
                       OR
                       (b.access_level='pro_adult' AND EXISTS (
                         SELECT 1 FROM user_level_grants ag
                         WHERE ag.user_id=acct.id AND ag.level='pro_adult'
                           AND ag.is_active
                       ))
                     )
               ))",
        )
        .bind(chapter_id)
        .bind(session_digest.map(|digest| digest.as_slice()))
        .fetch_optional(&self.pool)
        .await?;
        row.map(decode).transpose()
    }

    /// Saved historical FREE V1: pinned identity plus the SAME saved age,
    /// deletion and moderation policy as for_saved_session. An arbitrary
    /// version parameter is not sufficient, and anonymous mode is refused.
    pub async fn for_saved_active_pack_session(
        &self,
        chapter: i64,
        version: i64,
        digest: Option<&[u8; 32]>,
    ) -> Result<Option<FreeS3Ref>, sqlx::Error> {
        let Some(digest) = digest else {
            return Ok(None);
        };
        if chapter <= 0 || version <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT g.chapter_id,g.version_id,g.content_sha256 AS sha256,
                    g.size_bytes,pack.bucket
             FROM reader_saved_free_chapters pin
             JOIN chapters c ON c.id=pin.chapter_id
             JOIN books b ON b.id=c.book_id
             JOIN users u ON u.id=pin.user_id AND u.account_state='active'
             JOIN sessions sess ON sess.user_id=u.id AND sess.token_hash=$3::bytea
               AND sess.revoked_at IS NULL AND sess.expires_at>now()
             JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=pin.version_id
             JOIN free_edition_active_storage a
               ON a.chapter_id=v.chapter_id AND a.version_id=v.id
             JOIN free_edition_storage_generations g
               ON g.chapter_id=a.chapter_id AND g.version_id=a.version_id
              AND g.generation=a.generation AND g.placement_kind='pack'
              AND g.content_sha256=v.content_sha256
             JOIN free_storage_packs pack ON pack.id=g.pack_id
             WHERE pin.chapter_id=$1 AND pin.version_id=$2
               AND c.visibility='public' AND c.access_level='free'
               AND c.price_amount=0
               AND b.visibility_status='active' AND NOT b.admin_hold
               AND NOT b.is_deleted AND NOT c.admin_hold AND NOT c.is_deleted
               AND (b.visibility='public' OR b.lifecycle_status='pending_deletion'
                 OR (b.lifecycle_status='ordinary_deleted'
                   AND b.ordinary_deleted_at IS NOT NULL))
               AND (b.access_level='basic'
                 OR (b.access_level='adult' AND u.adult_enabled)
                 OR (b.access_level='pro_adult' AND EXISTS (
                   SELECT 1 FROM user_level_grants ug
                   WHERE ug.user_id=u.id AND ug.level='pro_adult'
                     AND ug.is_active)))",
        )
        .bind(chapter)
        .bind(version)
        .bind(digest.as_slice())
        .fetch_optional(&self.pool)
        .await?;
        row.map(decode).transpose()
    }
}

fn decode(row: sqlx::postgres::PgRow) -> Result<FreeS3Ref, sqlx::Error> {
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
}
