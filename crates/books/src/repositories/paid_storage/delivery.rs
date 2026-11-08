//! Resolve historical paid versions after a D purchase authorization.
use super::PgPaidStorage;
use async_trait::async_trait;
use sqlx::Row;
use tor_contracts::paid_delivery::{
    ChapterVersionPort, ChapterVersionRef, ContentCodec, DeliveryError, EncryptedObjectKind,
    EncryptedObjectRef, ObjectId, ObjectRef,
};
/// B resolves historical versions only after D returns an Allowed pinned
/// edition. No query joins the current-publication pointer: V1 is retained
/// after a later V2. Creator-hide is not refund; holds and hard deletion
/// always deny. The D port must independently authorize exact V1 first.
#[async_trait]
impl ChapterVersionPort for PgPaidStorage {
    async fn encrypted_chapter(
        &self,
        pinned: ChapterVersionRef,
    ) -> Result<Option<EncryptedObjectRef>, DeliveryError> {
        let row = sqlx::query(
            "SELECT o.id::text AS object_id, o.sha256_ciphertext, o.size_bytes,
                    o.object_nonce, o.object_aad, o.kek_id
             FROM chapter_versions v
             JOIN chapters c ON c.id=v.chapter_id
             JOIN books b ON b.id=c.book_id
             JOIN storage_objects o ON o.id=v.storage_object_id
             WHERE v.chapter_id=$1 AND v.id=$2
               AND c.access_level='paid'
               AND NOT c.admin_hold AND NOT c.is_deleted
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
               AND o.state='attached' AND o.object_kind='chapter' AND o.codec='gzip'",
        )
        .bind(pinned.chapter_id.get())
        .bind(pinned.version_id.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| DeliveryError::Unavailable)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let uuid: String = row
            .try_get("object_id")
            .map_err(|_| DeliveryError::Unavailable)?;
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
        let object_id = ObjectId::parse(&uuid).map_err(|_| DeliveryError::Unavailable)?;
        let object = ObjectRef::new(
            object_id,
            EncryptedObjectKind::Chapter,
            digest.try_into().map_err(|_| DeliveryError::Unavailable)?,
            size.try_into().map_err(|_| DeliveryError::Unavailable)?,
        )
        .map_err(|_| DeliveryError::Unavailable)?;
        Ok(Some(EncryptedObjectRef {
            object,
            nonce: nonce.try_into().map_err(|_| DeliveryError::Unavailable)?,
            aad,
            wrapped_key_id: kek_id,
            codec: ContentCodec::Gzip,
        }))
    }
}
