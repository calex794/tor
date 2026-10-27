//! G-09/S3-18: return a *scoped* paid ciphertext grant, never the global KEK.
//!
//! This is an internal port. C must first get D Allowed(pinned V1), then B's
//! exact immutable object and re-check D after this method returns. No public
//! HTTP endpoint, arbitrary S3 key, unverified HEAD or plaintext fallback.
use crate::{
    domain::{
        object_keys::PrivateObjectKey,
        provider::{valid_ciphertext_size, valid_private_bucket},
    },
    services::{
        crypto::{
            asset_aad, chapter_aad, unwrap_registered_dek_from_keyring, ContentKek, ContentKeyring,
        },
        provider::PrivateObjectStore,
    },
};
use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sqlx::{PgPool, Row};
use std::{sync::Arc, time::Duration};
use tor_contracts::paid_delivery::{
    validate_signed_get_ttl, ChapterVersionRef, ContentCodec, DeliveryError, EncryptedObjectKind,
    EncryptedObjectRef, GrantedContent, ObjectId, PositiveId, PrivateObjectDeliveryPort,
    SecretValue,
};

pub struct PgPrivateDelivery<S> {
    pool: PgPool,
    store: S,
    keyring: Arc<ContentKeyring>,
    private_bucket: String,
}

impl<S: PrivateObjectStore> PgPrivateDelivery<S> {
    /// Pass the same server-owned private bucket used to configure S3PrivateStore.
    /// A PG registry row from another bucket must never authorize this signer.
    pub fn new(
        pool: PgPool,
        store: S,
        kek: Arc<ContentKek>,
        private_bucket: String,
    ) -> Result<Self, DeliveryError> {
        let keyring = ContentKeyring::from_v1_and_optional_v2_environment(kek)
            .map_err(|_| DeliveryError::Unavailable)?;
        Self::with_keyring(pool, store, Arc::new(keyring), private_bucket)
    }

    /// G reads the trusted row's KEK identifier; a missing historical key
    /// denies the grant before any signed S3 URL is issued.
    pub fn with_keyring(
        pool: PgPool,
        store: S,
        keyring: Arc<ContentKeyring>,
        private_bucket: String,
    ) -> Result<Self, DeliveryError> {
        if !valid_private_bucket(&private_bucket) {
            return Err(DeliveryError::Invalid);
        }
        Ok(Self {
            pool,
            store,
            keyring,
            private_bucket,
        })
    }
}

fn canonical_aad(
    aad: &[u8],
    id: &ObjectId,
    plaintext_sha256: &[u8],
    kind: EncryptedObjectKind,
) -> bool {
    let Ok(digest) = <[u8; 32]>::try_from(plaintext_sha256) else {
        return false;
    };
    let Ok(text) = std::str::from_utf8(aad) else {
        return false;
    };
    let mut fields = text.split('\n');
    let namespace = fields.next();
    if namespace
        != Some(match kind {
            EncryptedObjectKind::Chapter => "tor.encrypted.chapter.v1",
            EncryptedObjectKind::ChapterAsset => "tor.encrypted.asset.v1",
        })
    {
        return false;
    }
    let parse_id = |raw: Option<&str>| {
        raw.and_then(|value| value.parse::<i64>().ok())
            .and_then(|value| PositiveId::new(value).ok())
    };
    let (Some(chapter_id), Some(version_id)) = (parse_id(fields.next()), parse_id(fields.next()))
    else {
        return false;
    };
    let pinned = ChapterVersionRef {
        chapter_id,
        version_id,
    };
    let canonical = match kind {
        EncryptedObjectKind::Chapter => chapter_aad(pinned, id, &digest),
        EncryptedObjectKind::ChapterAsset => asset_aad(pinned, id, &digest),
    };
    canonical == aad
}

#[async_trait]
impl<S: PrivateObjectStore> PrivateObjectDeliveryPort for PgPrivateDelivery<S> {
    async fn grant_verified_object(
        &self,
        encrypted: &EncryptedObjectRef,
        ttl: Duration,
    ) -> Result<GrantedContent, DeliveryError> {
        let (kind, codec) = match (encrypted.object.kind, encrypted.codec) {
            (EncryptedObjectKind::Chapter, ContentCodec::Gzip) => ("chapter", "gzip"),
            (EncryptedObjectKind::ChapterAsset, ContentCodec::None) => ("chapter_asset", "none"),
            _ => return Err(DeliveryError::Invalid),
        };
        if validate_signed_get_ttl(ttl).is_err()
            || (kind == "chapter_asset" && encrypted.object.size_bytes > 8 * 1024 * 1024)
            || !valid_ciphertext_size(&encrypted.object)
            || encrypted.aad.is_empty()
            || !matches!(
                encrypted.wrapped_key_id.as_str(),
                "content-kek-v1" | "content-kek-v2"
            )
        {
            return Err(DeliveryError::Invalid);
        }

        // The whole trusted B ObjectRef must match a G attached registry
        // row. A pending, ready, deleted, rekeyed or tampered row cannot sign.
        let row = sqlx::query(
            "SELECT encrypted_dek, wrap_nonce, bucket, object_key, plaintext_sha256
             FROM storage_objects
             WHERE id=$1::uuid AND object_kind=$7
               AND state='attached' AND codec=$8
               AND size_bytes=$2 AND sha256_ciphertext=$3
               AND object_nonce=$4 AND object_aad=$5 AND kek_id=$6
               AND ($7 <> 'chapter_asset' OR EXISTS (
                    SELECT 1 FROM chapter_version_assets AS image
                    JOIN chapter_versions AS v
                      ON v.chapter_id=image.chapter_id AND v.id=image.version_id
                    JOIN storage_objects AS body
                      ON body.id=v.storage_object_id
                    JOIN chapters AS c ON c.id=image.chapter_id
                    JOIN books AS b ON b.id=c.book_id
                    JOIN chapter_encrypted_release_events AS released
                      ON released.chapter_id=image.chapter_id
                     AND released.version_id=image.version_id
                     AND released.storage_object_id=body.id
                    WHERE image.storage_object_id=storage_objects.id
                      AND body.state='attached'
                      AND body.object_kind='chapter'
                      AND c.access_level='paid'
                      AND NOT c.admin_hold AND NOT c.is_deleted
                      AND NOT b.admin_hold AND NOT b.is_deleted
                      AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
               ))",
        )
        .bind(encrypted.object.object_id.as_str())
        .bind(encrypted.object.size_bytes as i64)
        .bind(encrypted.object.sha256_ciphertext.as_slice())
        .bind(encrypted.nonce.as_slice())
        .bind(&encrypted.aad)
        .bind(&encrypted.wrapped_key_id)
        .bind(kind)
        .bind(codec)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| DeliveryError::Unavailable)?
        .ok_or(DeliveryError::Unavailable)?;

        let bucket: String = row
            .try_get("bucket")
            .map_err(|_| DeliveryError::Unavailable)?;
        let key: String = row
            .try_get("object_key")
            .map_err(|_| DeliveryError::Unavailable)?;
        let plaintext_sha256: Vec<u8> = row
            .try_get("plaintext_sha256")
            .map_err(|_| DeliveryError::Unavailable)?;
        let expected_key =
            PrivateObjectKey::for_object(encrypted.object.kind, &encrypted.object.object_id);
        if bucket != self.private_bucket
            || key != expected_key.as_str()
            || !canonical_aad(
                &encrypted.aad,
                &encrypted.object.object_id,
                &plaintext_sha256,
                encrypted.object.kind,
            )
        {
            return Err(DeliveryError::Unavailable);
        }

        let wrapped: Vec<u8> = row
            .try_get("encrypted_dek")
            .map_err(|_| DeliveryError::Unavailable)?;
        let wrap_nonce: Vec<u8> = row
            .try_get("wrap_nonce")
            .map_err(|_| DeliveryError::Unavailable)?;
        let wrap_nonce: [u8; 12] = wrap_nonce
            .try_into()
            .map_err(|_| DeliveryError::Unavailable)?;

        let scoped_dek = unwrap_registered_dek_from_keyring(
            &encrypted.object.object_id,
            &wrapped,
            &wrap_nonce,
            &encrypted.wrapped_key_id,
            &self.keyring,
        )
        .map_err(|_| DeliveryError::Unavailable)?;

        // A trusted S3 provider verifies private bucket/key and real HEAD
        // metadata before signing. No credentials or signed URLs are logged.
        let ciphertext_url = self
            .store
            .presign_verified(&encrypted.object, ttl)
            .await
            .map_err(|_| DeliveryError::Unavailable)?;
        let dek_b64u = SecretValue::new(URL_SAFE_NO_PAD.encode(&scoped_dek[..]))
            .map_err(|_| DeliveryError::Unavailable)?;

        Ok(GrantedContent {
            ciphertext_url,
            object_sha256: hex::encode(encrypted.object.sha256_ciphertext),
            cipher: "AES-256-GCM",
            codec: encrypted.codec.wire(),
            nonce_b64u: URL_SAFE_NO_PAD.encode(encrypted.nonce),
            aad_b64u: URL_SAFE_NO_PAD.encode(&encrypted.aad),
            dek_b64u,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g_s3_20_rejects_noncanonical_or_different_object_aad() {
        let id = ObjectId::parse("a01e4711-2119-48a1-b280-74b38ac1b9a2").unwrap();
        let digest = [0x42_u8; 32];
        let pinned = ChapterVersionRef {
            chapter_id: PositiveId::new(41).unwrap(),
            version_id: PositiveId::new(42).unwrap(),
        };
        let canonical = chapter_aad(pinned, &id, &digest);
        assert!(canonical_aad(&canonical, &id, &digest, EncryptedObjectKind::Chapter));
        assert!(!canonical_aad(
            &canonical,
            &id,
            &[0_u8; 32],
            EncryptedObjectKind::Chapter,
        ));
        let swapped = ObjectId::parse("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb").unwrap();
        assert!(!canonical_aad(
            &canonical,
            &swapped,
            &digest,
            EncryptedObjectKind::Chapter,
        ));
        let padded = String::from_utf8(canonical)
            .unwrap()
            .replace("\n41\n", "\n041\n");
        assert!(!canonical_aad(
            padded.as_bytes(),
            &id,
            &digest,
            EncryptedObjectKind::Chapter,
        ));
        let image = asset_aad(pinned, &id, &digest);
        assert!(canonical_aad(
            &image,
            &id,
            &digest,
            EncryptedObjectKind::ChapterAsset,
        ));
        assert!(!canonical_aad(
            &image,
            &id,
            &digest,
            EncryptedObjectKind::Chapter,
        ));
    }
}
