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
        crypto::{chapter_aad, unwrap_registered_dek, ContentKek},
        provider::PrivateObjectStore,
    },
};
use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sqlx::{PgPool, Row};
use std::{sync::Arc, time::Duration};
use tor_contracts::paid_delivery::{
    validate_signed_get_ttl, ChapterVersionRef, ContentCodec, DeliveryError, EncryptedObjectKind,
    EncryptedObjectRef, GrantedContent, ObjectId, PositiveId, PrivateObjectDeliveryPort, SecretValue,
};

pub struct PgPrivateDelivery<S> {
    pool: PgPool,
    store: S,
    kek: Arc<ContentKek>,
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
        if !valid_private_bucket(&private_bucket) {
            return Err(DeliveryError::Invalid);
        }
        Ok(Self {
            pool,
            store,
            kek,
            private_bucket,
        })
    }
}

fn canonical_aad(aad: &[u8], id: &ObjectId, plaintext_sha256: &[u8]) -> bool {
    let Ok(digest): Result<[u8; 32], _> = plaintext_sha256.try_into() else {
        return false;
    };
    let Ok(text) = std::str::from_utf8(aad) else {
        return false;
    };
    let mut fields = text.split('\n');
    if fields.next() != Some("tor.encrypted.chapter.v1") {
        return false;
    }
    let parse_id = |raw: Option<&str>| {
        raw.and_then(|value| value.parse::<i64>().ok())
            .and_then(|value| PositiveId::new(value).ok())
    };
    let (Some(chapter_id), Some(version_id)) =
        (parse_id(fields.next()), parse_id(fields.next()))
    else {
        return false;
    };
    chapter_aad(
        ChapterVersionRef {
            chapter_id,
            version_id,
        },
        id,
        &digest,
    ) == aad
}

#[async_trait]
impl<S: PrivateObjectStore> PrivateObjectDeliveryPort for PgPrivateDelivery<S> {
    async fn grant_verified_object(
        &self,
        encrypted: &EncryptedObjectRef,
        ttl: Duration,
    ) -> Result<GrantedContent, DeliveryError> {
        if validate_signed_get_ttl(ttl).is_err()
            || encrypted.object.kind != EncryptedObjectKind::Chapter
            || encrypted.codec != ContentCodec::Gzip
            || !valid_ciphertext_size(&encrypted.object)
            || encrypted.aad.is_empty()
            || encrypted.wrapped_key_id != "content-kek-v1"
        {
            return Err(DeliveryError::Invalid);
        }

        // The whole trusted B ObjectRef must match a G attached registry
        // row. A pending, ready, deleted, rekeyed or tampered row cannot sign.
        let row = sqlx::query(
            "SELECT encrypted_dek, wrap_nonce, bucket, object_key, plaintext_sha256
             FROM storage_objects
             WHERE id=$1::uuid AND object_kind='chapter'
               AND state='attached' AND codec='gzip'
               AND size_bytes=$2 AND sha256_ciphertext=$3
               AND object_nonce=$4 AND object_aad=$5 AND kek_id=$6",
        )
        .bind(encrypted.object.object_id.as_str())
        .bind(encrypted.object.size_bytes as i64)
        .bind(encrypted.object.sha256_ciphertext.as_slice())
        .bind(encrypted.nonce.as_slice())
        .bind(&encrypted.aad)
        .bind(&encrypted.wrapped_key_id)
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

        let scoped_dek = unwrap_registered_dek(
            &encrypted.object.object_id,
            &wrapped,
            &wrap_nonce,
            &encrypted.wrapped_key_id,
            &self.kek,
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
        assert!(canonical_aad(&canonical, &id, &digest));
        assert!(!canonical_aad(&canonical, &id, &[0_u8; 32]));
        let swapped = ObjectId::parse("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb").unwrap();
        assert!(!canonical_aad(&canonical, &swapped, &digest));
        let padded = String::from_utf8(canonical)
            .unwrap()
            .replace("\n41\n", "\n041\n");
        assert!(!canonical_aad(padded.as_bytes(), &id, &digest));
    }
}
