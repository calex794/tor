//! B/S065: immutable paid-image membership, never a reader entitlement.
//! Only G-verified, exact-version assets may be attached to an unpublished
//! edition. C must resolve D rights before requesting any B asset reference.
use crate::services::authoring::AuthorError;
use sqlx::{PgPool, Row};
use tor_contracts::paid_delivery::{
    ChapterVersionRef, ContentCodec, DeliveryError, EncryptedObjectKind, EncryptedObjectRef,
    ObjectId, ObjectRef,
};

fn write_error(error: sqlx::Error) -> AuthorError {
    if let sqlx::Error::Database(ref db) = error {
        if matches!(db.code().as_deref(), Some("23503" | "23505" | "23514")) {
            return AuthorError::Conflict;
        }
    }
    AuthorError::Unavailable
}

#[derive(Clone)]
pub struct PgPaidAssets {
    pool: PgPool,
}

impl PgPaidAssets {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Bind one preverified image to B's exact draft V1. The G object has
    /// already been uploaded and hash-verified; the B transaction either
    /// records the one-time association and attaches G or rolls both back.
    /// Existing associations can be replayed after publication, never changed.
    pub async fn attach_prepared(
        &self,
        actor: i64,
        pinned: ChapterVersionRef,
        ordinal: i16,
        media_type: &str,
        object_id: &ObjectId,
    ) -> Result<(), AuthorError> {
        if actor <= 0
            || !(1..=64).contains(&ordinal)
            || !matches!(media_type, "image/png" | "image/jpeg" | "image/webp")
        {
            return Err(AuthorError::Invalid);
        }
        let chapter = pinned.chapter_id.get();
        let version = pinned.version_id.get();
        let mut tx = self.pool.begin().await.map_err(write_error)?;
        // Use the same book -> chapter -> version lock order as B publication.
        let book: Option<i64> = sqlx::query_scalar(
            "SELECT b.id FROM books b JOIN chapters c ON c.book_id=b.id
             WHERE c.id=$1 AND b.owner_user_id=$2
               AND NOT b.admin_hold AND NOT b.is_deleted
             FOR UPDATE OF b",
        )
        .bind(chapter)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?;
        let Some(book) = book else {
            return Err(AuthorError::Missing);
        };
        let valid_chapter: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM chapters
             WHERE id=$1 AND book_id=$2 AND access_level='paid'
               AND price_amount>0 AND NOT admin_hold AND NOT is_deleted
             FOR UPDATE",
        )
        .bind(chapter)
        .bind(book)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?;
        if valid_chapter.is_none() {
            return Err(AuthorError::Missing);
        }
        let version_storage: Option<Option<String>> = sqlx::query_scalar(
            "SELECT storage_object_id::text FROM chapter_versions
             WHERE chapter_id=$1 AND id=$2 FOR UPDATE",
        )
        .bind(chapter)
        .bind(version)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?;
        let Some(version_storage) = version_storage else {
            return Err(AuthorError::Missing);
        };

        let existing: Option<(String, String)> = sqlx::query_as(
            "SELECT storage_object_id::text,media_type
             FROM chapter_version_assets
             WHERE chapter_id=$1 AND version_id=$2 AND ordinal=$3",
        )
        .bind(chapter)
        .bind(version)
        .bind(ordinal)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?;
        if let Some((id, mime)) = existing {
            return if id == object_id.as_str() && mime == media_type {
                Ok(())
            } else {
                Err(AuthorError::Conflict)
            };
        }
        // No new images may enter an edition after its chapter object was
        // attached or published. Readers of purchased V1 retain fixed bytes.
        let released: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM chapter_publications
                WHERE chapter_id=$1 AND version_id=$2
             ) OR EXISTS(
                SELECT 1 FROM chapter_encrypted_release_events
                WHERE chapter_id=$1 AND version_id=$2
             )",
        )
        .bind(chapter)
        .bind(version)
        .fetch_one(&mut *tx)
        .await
        .map_err(write_error)?;
        if released || version_storage.is_some() {
            return Err(AuthorError::Conflict);
        }

        let attached: Option<String> = sqlx::query_scalar(
            "UPDATE storage_objects
             SET state='attached',attached_at=now(),delete_after=NULL
             WHERE id=$1::uuid AND state='ready'
               AND object_kind='chapter_asset' AND codec='none'
               AND object_aad=convert_to(
                  'tor.encrypted.asset.v1' || E'\n' ||
                  $2::text || E'\n' || $3::text || E'\n' ||
                  $1::text || E'\n' ||
                  encode(plaintext_sha256,'hex') || E'\n','UTF8'
               )
             RETURNING id::text",
        )
        .bind(object_id.as_str())
        .bind(chapter)
        .bind(version)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?;
        if attached.as_deref() != Some(object_id.as_str()) {
            return Err(AuthorError::Conflict);
        }
        sqlx::query(
            "INSERT INTO chapter_version_assets
             (chapter_id,version_id,storage_object_id,ordinal,media_type)
             VALUES($1,$2,$3::uuid,$4,$5)",
        )
        .bind(chapter)
        .bind(version)
        .bind(object_id.as_str())
        .bind(ordinal)
        .bind(media_type)
        .execute(&mut *tx)
        .await
        .map_err(write_error)?;
        tx.commit().await.map_err(write_error)?;
        Ok(())
    }

    /// B resolves only a verified image of the exact immutable edition.
    /// This method is not an entitlement; C must confirm D before use.
    pub async fn encrypted_for_pinned(
        &self,
        pinned: ChapterVersionRef,
        object_id: &ObjectId,
    ) -> Result<Option<EncryptedObjectRef>, DeliveryError> {
        let row = sqlx::query(
            "SELECT o.sha256_ciphertext,o.size_bytes,o.object_nonce,
                    o.object_aad,o.kek_id
             FROM chapter_version_assets a
             JOIN chapter_versions v
               ON v.chapter_id=a.chapter_id AND v.id=a.version_id
             JOIN chapters c ON c.id=a.chapter_id
             JOIN books b ON b.id=c.book_id
             JOIN storage_objects o ON o.id=a.storage_object_id
             WHERE a.chapter_id=$1 AND a.version_id=$2
               AND a.storage_object_id=$3::uuid
               AND v.storage_object_id IS NOT NULL
               AND c.access_level='paid'
               AND NOT c.admin_hold AND NOT c.is_deleted
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
               AND o.state='attached' AND o.object_kind='chapter_asset'
               AND o.codec='none'
               AND o.object_aad=convert_to(
                  'tor.encrypted.asset.v1' || E'\n' ||
                  a.chapter_id::text || E'\n' ||
                  a.version_id::text || E'\n' ||
                  o.id::text || E'\n' ||
                  encode(o.plaintext_sha256,'hex') || E'\n','UTF8'
               )",
        )
        .bind(pinned.chapter_id.get())
        .bind(pinned.version_id.get())
        .bind(object_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| DeliveryError::Unavailable)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let size: i64 = row
            .try_get("size_bytes")
            .map_err(|_| DeliveryError::Unavailable)?;
        let digest: Vec<u8> = row
            .try_get("sha256_ciphertext")
            .map_err(|_| DeliveryError::Unavailable)?;
        let nonce: Vec<u8> = row
            .try_get("object_nonce")
            .map_err(|_| DeliveryError::Unavailable)?;
        let aad: Vec<u8> = row
            .try_get("object_aad")
            .map_err(|_| DeliveryError::Unavailable)?;
        let kek_id: String = row
            .try_get("kek_id")
            .map_err(|_| DeliveryError::Unavailable)?;
        let object = ObjectRef::new(
            object_id.clone(),
            EncryptedObjectKind::ChapterAsset,
            digest.try_into().map_err(|_| DeliveryError::Unavailable)?,
            size.try_into().map_err(|_| DeliveryError::Unavailable)?,
        )
        .map_err(|_| DeliveryError::Unavailable)?;
        Ok(Some(EncryptedObjectRef {
            object,
            nonce: nonce.try_into().map_err(|_| DeliveryError::Unavailable)?,
            aad,
            wrapped_key_id: kek_id,
            codec: ContentCodec::None,
        }))
    }
}
