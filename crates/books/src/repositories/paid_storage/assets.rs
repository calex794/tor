//! B S065: trusted immutable image membership for an exact paid edition.
//! No payment decision is made here; C always checks D before granting G.
use super::*;
use tor_contracts::paid_delivery::ChapterAssetPort;

impl PgPaidStorage {
    /// Only the book owner can associate a verified ready private object with
    /// an already attached paid version. No free/public image URL is created.
    /// The caller must have validated the original image MIME and bytes.
    pub async fn attach_verified_asset(
        &self,
        actor: i64,
        pinned: ChapterVersionRef,
        object_id: &ObjectId,
        mime: &str,
        ordinal: i32,
    ) -> Result<(), AuthorError> {
        if actor <= 0
            || !(1..=200).contains(&ordinal)
            || !matches!(mime, "image/png" | "image/jpeg" | "image/webp")
        {
            return Err(AuthorError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(map_write)?;
        // Lock the paid B edition and owner; an unpublished historical V1 is
        // still valid if its attached ciphertext survives a V2 publication.
        let owner: Option<i64> = sqlx::query_scalar(
            "SELECT b.owner_user_id
             FROM chapter_versions v
             JOIN chapters c ON c.id=v.chapter_id
             JOIN books b ON b.id=c.book_id
             JOIN storage_objects body_o
               ON body_o.id=v.storage_object_id
             WHERE v.chapter_id=$1 AND v.id=$2
               AND c.access_level='paid'
               AND NOT c.admin_hold AND NOT c.is_deleted
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND body_o.state='attached' AND body_o.object_kind='chapter'
             FOR UPDATE OF v,c,b",
        )
        .bind(pinned.chapter_id.get())
        .bind(pinned.version_id.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_write)?;
        if owner != Some(actor) {
            return Err(AuthorError::Missing);
        }

        let previous: Option<(i64, i64, i32, String)> = sqlx::query(
            "SELECT chapter_id,version_id,ordinal,kind
             FROM chapter_version_assets
             WHERE storage_object_id=$1::uuid FOR UPDATE",
        )
        .bind(object_id.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_write)?
        .map(|row| {
            Ok((
                row.try_get("chapter_id").map_err(|_| AuthorError::Unavailable)?,
                row.try_get("version_id").map_err(|_| AuthorError::Unavailable)?,
                row.try_get("ordinal").map_err(|_| AuthorError::Unavailable)?,
                row.try_get("kind").map_err(|_| AuthorError::Unavailable)?,
            ))
        })
        .transpose()?;
        if let Some((chapter, version, old_ordinal, old_mime)) = previous {
            return if chapter == pinned.chapter_id.get()
                && version == pinned.version_id.get()
                && old_ordinal == ordinal
                && old_mime == mime
            {
                Ok(())
            } else {
                Err(AuthorError::Conflict)
            };
        }

        // This ready G object must be exactly the media envelope for pinned V1.
        // B never uses an arbitrary path/host supplied by a reader or uploader.
        let ready: Option<String> = sqlx::query_scalar(
            "UPDATE storage_objects
             SET state='attached',attached_at=now(),delete_after=NULL
             WHERE id=$1::uuid AND state='ready'
               AND object_kind='chapter_asset' AND codec='none'
               AND size_bytes BETWEEN 17 AND 8388608
               AND object_aad=convert_to(
                  'tor.encrypted.asset.v1' || E'\\n' ||
                  $2::text || E'\\n' || $3::text || E'\\n' ||
                  id::text || E'\\n' ||
                  encode(plaintext_sha256,'hex') || E'\\n',
                  'UTF8'
               )
             RETURNING id::text",
        )
        .bind(object_id.as_str())
        .bind(pinned.chapter_id.get())
        .bind(pinned.version_id.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_write)?;
        if ready.is_none() {
            return Err(AuthorError::Conflict);
        }
        sqlx::query(
            "INSERT INTO chapter_version_assets (
                chapter_id,version_id,storage_object_id,kind,ordinal
             ) VALUES($1,$2,$3::uuid,$4,$5)",
        )
        .bind(pinned.chapter_id.get())
        .bind(pinned.version_id.get())
        .bind(object_id.as_str())
        .bind(mime)
        .bind(ordinal)
        .execute(&mut *tx)
        .await
        .map_err(map_write)?;
        tx.commit().await.map_err(map_write)?;
        Ok(())
    }
}

#[async_trait]
impl ChapterAssetPort for PgPaidStorage {
    async fn encrypted_asset(
        &self,
        pinned: ChapterVersionRef,
        requested: &ObjectId,
    ) -> Result<Option<EncryptedObjectRef>, DeliveryError> {
        let row = sqlx::query(
            "SELECT o.id::text AS object_id,o.sha256_ciphertext,o.size_bytes,
                    o.object_nonce,o.object_aad,o.kek_id
             FROM chapter_version_assets a
             JOIN chapter_versions v ON v.chapter_id=a.chapter_id
                                    AND v.id=a.version_id
             JOIN chapters c ON c.id=a.chapter_id
             JOIN books b ON b.id=c.book_id
             JOIN storage_objects body_o
               ON body_o.id=v.storage_object_id AND body_o.state='attached'
             JOIN storage_objects o ON o.id=a.storage_object_id
             WHERE a.chapter_id=$1 AND a.version_id=$2
               AND a.storage_object_id=$3::uuid
               AND c.access_level='paid'
               AND NOT c.admin_hold AND NOT c.is_deleted
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
               AND o.state='attached' AND o.object_kind='chapter_asset'
               AND o.codec='none'",
        )
        .bind(pinned.chapter_id.get())
        .bind(pinned.version_id.get())
        .bind(requested.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| DeliveryError::Unavailable)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let digest: Vec<u8> = row.try_get("sha256_ciphertext")
            .map_err(|_| DeliveryError::Unavailable)?;
        let size: i64 = row.try_get("size_bytes")
            .map_err(|_| DeliveryError::Unavailable)?;
        let nonce: Vec<u8> = row.try_get("object_nonce")
            .map_err(|_| DeliveryError::Unavailable)?;
        Ok(Some(EncryptedObjectRef {
            object: ObjectRef::new(
                requested.clone(),
                EncryptedObjectKind::ChapterAsset,
                digest.try_into().map_err(|_| DeliveryError::Unavailable)?,
                size.try_into().map_err(|_| DeliveryError::Unavailable)?,
            ).map_err(|_| DeliveryError::Unavailable)?,
            nonce: nonce.try_into().map_err(|_| DeliveryError::Unavailable)?,
            aad: row.try_get("object_aad").map_err(|_| DeliveryError::Unavailable)?,
            wrapped_key_id: row.try_get("kek_id")
                .map_err(|_| DeliveryError::Unavailable)?,
            codec: ContentCodec::None,
        }))
    }
}
