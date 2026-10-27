//! G S065: private image registration, separate from gzip chapter bodies.
use super::*;
use crate::services::crypto::{
    asset_aad, unwrap_registered_dek_from_keyring, EncryptedAsset,
};

impl PgStorageRegistry {
    /// Reserve only G-produced, authenticated bytes, never public or
    /// browser-chosen object keys. B attaches this verified ready row later.
    pub async fn reserve_pending_asset(
        &self,
        asset: &EncryptedAsset,
        keyring: &ContentKeyring,
        bucket: &str,
    ) -> Result<ObjectRef, RegistryError> {
        let object = ObjectRef::new(
            asset.object_id.clone(),
            EncryptedObjectKind::ChapterAsset,
            asset.ciphertext_sha256,
            asset.ciphertext.len() as u64,
        )
        .map_err(|_| RegistryError::Invalid)?;
        if !valid_private_bucket(bucket)
            || !valid_ciphertext_size(&object)
            || object.size_bytes > 8 * 1024 * 1024 + 16
            || !matches!(asset.mime, "image/png" | "image/jpeg" | "image/webp")
            || asset.kek_id != keyring.active_id()
            || asset.wrapped_dek.len() != 48
            || asset.aad != asset_aad(asset.version, &asset.object_id, &asset.plaintext_sha256)
            || sha256_bytes(&asset.ciphertext) != asset.ciphertext_sha256
            || unwrap_registered_dek_from_keyring(
                &asset.object_id,
                &asset.wrapped_dek,
                &asset.wrap_nonce,
                asset.kek_id,
                keyring,
            ).is_err()
        {
            return Err(RegistryError::Invalid);
        }
        let key = PrivateObjectKey::for_object(object.kind, &object.object_id);
        let created: Option<String> = sqlx::query_scalar(
            "INSERT INTO storage_objects(
                id,object_kind,bucket,object_key,state,size_bytes,
                sha256_ciphertext,plaintext_sha256,encrypted_dek,wrap_nonce,
                kek_id,object_nonce,object_aad,codec,delete_after
             ) VALUES(
                $1::uuid,'chapter_asset',$2,$3,'pending',$4,
                $5,$6,$7,$8,$9,$10,$11,'none',now()+INTERVAL '24 hours'
             ) ON CONFLICT (id) DO NOTHING RETURNING id::text",
        )
        .bind(asset.object_id.as_str())
        .bind(bucket)
        .bind(key.as_str())
        .bind(object.size_bytes as i64)
        .bind(asset.ciphertext_sha256.as_slice())
        .bind(asset.plaintext_sha256.as_slice())
        .bind(&asset.wrapped_dek)
        .bind(asset.wrap_nonce.as_slice())
        .bind(asset.kek_id)
        .bind(asset.object_nonce.as_slice())
        .bind(&asset.aad)
        .fetch_optional(&self.pool)
        .await
        .map_err(sql_error)?;
        if created.is_none() {
            return Err(RegistryError::Conflict);
        }
        Ok(object)
    }
}
