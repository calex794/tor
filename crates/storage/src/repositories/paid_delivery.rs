//! G-09/S3-18: return a *scoped* paid ciphertext grant, never the global KEK.
//!
//! This is an internal port. C must first get D Allowed(pinned V1), then B's
//! exact immutable object and re-check D after this method returns. No public
//! HTTP endpoint, arbitrary S3 key, unverified HEAD or plaintext fallback.
use crate::{
    domain::provider::valid_ciphertext_size,
    services::{
        crypto::{unwrap_registered_dek, ContentKek},
        provider::PrivateObjectStore,
    },
};
use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sqlx::{PgPool, Row};
use std::{sync::Arc, time::Duration};
use tor_contracts::paid_delivery::{
    validate_signed_get_ttl, ContentCodec, DeliveryError, EncryptedObjectKind, EncryptedObjectRef,
    GrantedContent, PrivateObjectDeliveryPort, SecretValue,
};

pub struct PgPrivateDelivery<S> {
    pool: PgPool,
    store: S,
    kek: Arc<ContentKek>,
}

impl<S: PrivateObjectStore> PgPrivateDelivery<S> {
    pub fn new(pool: PgPool, store: S, kek: Arc<ContentKek>) -> Self {
        Self { pool, store, kek }
    }
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
            "SELECT encrypted_dek, wrap_nonce
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
