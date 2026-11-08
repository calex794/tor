//! B/S065: the same G-verified asset membership under a verified
//! administrative reading authority. Ordinary buyer guards remain separate.
use async_trait::async_trait;
use sqlx::{PgPool, Row};
use tor_contracts::paid_delivery::{
    ChapterAssetPort, ChapterVersionRef, ContentCodec, DeliveryError, EncryptedObjectKind,
    EncryptedObjectRef, ObjectId, ObjectRef,
};

#[derive(Clone)]
pub struct PgAdminPaidAssets {
    pool: PgPool,
}

impl PgAdminPaidAssets {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

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

#[async_trait]
impl ChapterAssetPort for PgAdminPaidAssets {
    async fn encrypted_asset(
        &self,
        pinned: ChapterVersionRef,
        object_id: &ObjectId,
    ) -> Result<Option<EncryptedObjectRef>, DeliveryError> {
        self.encrypted_for_pinned(pinned, object_id).await
    }
}
