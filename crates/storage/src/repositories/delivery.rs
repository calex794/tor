//! G S3-20: checked, short-lived S3 ciphertext grant for a B-resolved object.
//! This adapter does not decide buyer rights: I must obtain fresh D Allowed
//! and B's exact pinned object BEFORE calling this G port.
use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sqlx::{PgPool, Row};
use std::time::Duration;
use tor_contracts::paid_delivery::{
    validate_signed_get_ttl, ContentCodec, DeliveryError, EncryptedObjectKind, EncryptedObjectRef,
    GrantedContent, PrivateObjectDeliveryPort, SecretValue,
};

use crate::{
    domain::{
        object_keys::PrivateObjectKey,
        provider::{valid_ciphertext_size, valid_private_bucket},
    },
    services::{
        crypto::{unwrap_registered_dek, ContentKek},
        provider::PrivateObjectStore,
    },
};

/// The bucket and KEK are trusted server configuration, never request fields.
pub struct PgPrivateDelivery<S> {
    pool: PgPool,
    store: S,
    kek: ContentKek,
    private_bucket: String,
}

impl<S: PrivateObjectStore> PgPrivateDelivery<S> {
    pub fn new(
        pool: PgPool,
        store: S,
        kek: ContentKek,
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

fn canonical_chapter_aad(aad: &[u8], object_id: &str, plaintext_sha256: &[u8]) -> bool {
    let Ok(value) = std::str::from_utf8(aad) else {
        return false;
    };
    if plaintext_sha256.len() != 32 {
        return false;
    }
    let digest = hex::encode(plaintext_sha256);
    let mut fields = value.split('\n');
    fields.next() == Some("tor.encrypted.chapter.v1")
        && fields
            .next()
            .and_then(|id| id.parse::<i64>().ok())
            .is_some_and(|id| id > 0)
        && fields
            .next()
            .and_then(|id| id.parse::<i64>().ok())
            .is_some_and(|id| id > 0)
        && fields.next() == Some(object_id)
        && fields.next() == Some(digest.as_str())
        && fields.next() == Some("")
        && fields.next().is_none()
}

#[async_trait]
impl<S: PrivateObjectStore + Send + Sync> PrivateObjectDeliveryPort for PgPrivateDelivery<S> {
    async fn grant_verified_object(
        &self,
        encrypted: &EncryptedObjectRef,
        ttl: Duration,
    ) -> Result<GrantedContent, DeliveryError> {
        if validate_signed_get_ttl(ttl).is_err()
            || !valid_ciphertext_size(&encrypted.object)
            || encrypted.object.kind != EncryptedObjectKind::Chapter
            || encrypted.codec != ContentCodec::Gzip
            || encrypted.wrapped_key_id != "content-kek-v1"
        {
            return Err(DeliveryError::Invalid);
        }

        let row = sqlx::query(
            "SELECT bucket,object_key,sha256_ciphertext,size_bytes,plaintext_sha256,
                    encrypted_dek,wrap_nonce,kek_id,object_nonce,object_aad
             FROM storage_objects
             WHERE id=$1::uuid AND state='attached'
               AND object_kind='chapter' AND codec='gzip'",
        )
        .bind(encrypted.object.object_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| DeliveryError::Unavailable)?
        .ok_or(DeliveryError::Hidden)?;

        // All metadata must match B's immutable object reference exactly.
        // SQL state alone never authorizes delivery or recovers plaintext.
        let bucket: String = row
            .try_get("bucket")
            .map_err(|_| DeliveryError::Unavailable)?;
        let key: String = row
            .try_get("object_key")
            .map_err(|_| DeliveryError::Unavailable)?;
        let size: i64 = row
            .try_get("size_bytes")
            .map_err(|_| DeliveryError::Unavailable)?;
        let ciphertext_sha256: Vec<u8> = row
            .try_get("sha256_ciphertext")
            .map_err(|_| DeliveryError::Unavailable)?;
        let plaintext_sha256: Vec<u8> = row
            .try_get("plaintext_sha256")
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
        let wrap_nonce: Vec<u8> = row
            .try_get("wrap_nonce")
            .map_err(|_| DeliveryError::Unavailable)?;
        let wrapped: Vec<u8> = row
            .try_get("encrypted_dek")
            .map_err(|_| DeliveryError::Unavailable)?;

        let expected_key =
            PrivateObjectKey::for_object(encrypted.object.kind, &encrypted.object.object_id);
        if bucket != self.private_bucket
            || key != expected_key.as_str()
            || size != encrypted.object.size_bytes as i64
            || ciphertext_sha256.as_slice() != encrypted.object.sha256_ciphertext.as_slice()
            || nonce.as_slice() != encrypted.nonce.as_slice()
            || aad.as_slice() != encrypted.aad.as_slice()
            || kek_id != encrypted.wrapped_key_id
            || !canonical_chapter_aad(&aad, encrypted.object.object_id.as_str(), &plaintext_sha256)
        {
            return Err(DeliveryError::Unavailable);
        }

        let dek = unwrap_registered_dek(
            &encrypted.object.object_id,
            &kek_id,
            &wrap_nonce,
            &wrapped,
            &self.kek,
        )
        .map_err(|_| DeliveryError::Unavailable)?;

        // G checks real S3 HEAD metadata before signing. The original S3 PUT
        // was verified by a bounded actual GET prior to PG Ready/Attached.
        // Inaccessible or corrupted storage MUST NOT produce a grant.
        let url = self
            .store
            .presign_verified(&encrypted.object, ttl)
            .await
            .map_err(|_| DeliveryError::Unavailable)?;
        let secret = SecretValue::new(URL_SAFE_NO_PAD.encode(dek.as_slice()))
            .map_err(|_| DeliveryError::Unavailable)?;
        Ok(GrantedContent {
            ciphertext_url: url,
            object_sha256: hex::encode(encrypted.object.sha256_ciphertext),
            cipher: "AES-256-GCM",
            codec: encrypted.codec.wire(),
            nonce_b64u: URL_SAFE_NO_PAD.encode(encrypted.nonce),
            aad_b64u: URL_SAFE_NO_PAD.encode(&encrypted.aad),
            dek_b64u: secret,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g_s3_20_canonical_aad_requires_fixed_version_object_and_digest() {
        let object_id = "a01e4711-2119-48a1-b280-74b38ac1b9a2";
        let digest = [0x42_u8; 32];
        let canonical = format!(
            "tor.encrypted.chapter.v1\n41\n42\n{object_id}\n{}\n",
            hex::encode(digest)
        );
        assert!(canonical_chapter_aad(
            canonical.as_bytes(),
            object_id,
            &digest
        ));
        assert!(!canonical_chapter_aad(
            canonical.replace("\n41\n", "\n0\n").as_bytes(),
            object_id,
            &digest
        ));
        assert!(!canonical_chapter_aad(
            canonical.as_bytes(),
            "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            &digest
        ));
        assert!(!canonical_chapter_aad(
            canonical.as_bytes(),
            object_id,
            &[0_u8; 32]
        ));
    }
}
