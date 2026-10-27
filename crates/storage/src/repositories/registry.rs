//! G/S3-08 PostgreSQL registry for immutable ciphertext, never paid plaintext.
//! A ready row is NOT a B published chapter or a D entitlement.
//! The only ready transition requires a G-owned verified GET/hash receipt.
use crate::{
    domain::{
        object_keys::PrivateObjectKey,
        provider::{valid_ciphertext_size, valid_private_bucket},
    },
    services::{
        crypto::{chapter_aad, sha256_bytes, unwrap_scoped_dek, ContentKek, EncryptedChapter},
        provider::VerifiedObject,
    },
};
use sqlx::{PgPool, Row};
use tor_contracts::paid_delivery::{EncryptedObjectKind, ObjectId, ObjectRef};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistryError {
    Invalid,
    Conflict,
    Missing,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectState {
    Pending,
    Ready,
    Attached,
    Deleting,
}

fn sql_error(error: sqlx::Error) -> RegistryError {
    if let sqlx::Error::Database(ref database) = error {
        match database.code().as_deref() {
            Some("23505") => return RegistryError::Conflict,
            Some("23514") => return RegistryError::Invalid,
            _ => {}
        }
    }
    RegistryError::Unavailable
}

#[derive(Clone)]
pub struct PgStorageRegistry {
    pool: PgPool,
}

impl PgStorageRegistry {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Reserve before S3 upload. Keys, AAD, wrapped DEK and digest are checked
    /// against the trusted G encryption envelope, not browser-provided data.
    pub async fn reserve_pending(
        &self,
        sealed: &EncryptedChapter,
        kek: &ContentKek,
        private_bucket: &str,
    ) -> Result<ObjectRef, RegistryError> {
        let bytes = &sealed.ciphertext;
        let object = ObjectRef::new(
            sealed.object_id.clone(),
            EncryptedObjectKind::Chapter,
            sealed.ciphertext_sha256,
            bytes.len() as u64,
        )
        .map_err(|_| RegistryError::Invalid)?;
        if !valid_private_bucket(private_bucket)
            || !valid_ciphertext_size(&object)
            || sha256_bytes(bytes) != sealed.ciphertext_sha256
            || sealed.aad
                != chapter_aad(sealed.version, &sealed.object_id, &sealed.plaintext_sha256)
            || sealed.wrapped_dek.len() != 48
            || sealed.kek_id != "content-kek-v1"
            || unwrap_scoped_dek(sealed, kek).is_err()
        {
            return Err(RegistryError::Invalid);
        }
        let key = PrivateObjectKey::for_object(object.kind, &object.object_id);
        let id: Option<String> = sqlx::query_scalar(
            "INSERT INTO storage_objects (
                id,object_kind,bucket,object_key,state,size_bytes,
                sha256_ciphertext,plaintext_sha256,encrypted_dek,wrap_nonce,
                kek_id,object_nonce,object_aad,codec,delete_after
             ) VALUES (
                $1::uuid,'chapter',$2,$3,'pending',$4,
                $5,$6,$7,$8,$9,$10,$11,'gzip',now()+INTERVAL '24 hours'
             ) ON CONFLICT (id) DO NOTHING RETURNING id::text",
        )
        .bind(object.object_id.as_str())
        .bind(private_bucket)
        .bind(key.as_str())
        .bind(object.size_bytes as i64)
        .bind(sealed.ciphertext_sha256.as_slice())
        .bind(sealed.plaintext_sha256.as_slice())
        .bind(&sealed.wrapped_dek)
        .bind(sealed.wrap_nonce.as_slice())
        .bind(sealed.kek_id)
        .bind(sealed.object_nonce.as_slice())
        .bind(&sealed.aad)
        .fetch_optional(&self.pool)
        .await
        .map_err(sql_error)?;
        if id.is_none() {
            return Err(RegistryError::Conflict);
        }
        Ok(object)
    }

    /// Requires the opaque receipt issued by upload_and_verify, which
    /// always compares actual S3 GET bytes against the immutable digest.
    pub async fn mark_ready(&self, receipt: &VerifiedObject) -> Result<(), RegistryError> {
        let object = receipt.object();
        let updated: Option<String> = sqlx::query_scalar(
            "UPDATE storage_objects SET state='ready',verified_at=now()
             WHERE id=$1::uuid AND state='pending' AND
                   size_bytes=$2 AND sha256_ciphertext=$3
             RETURNING id::text",
        )
        .bind(object.object_id.as_str())
        .bind(object.size_bytes as i64)
        .bind(object.sha256_ciphertext.as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(sql_error)?;
        if updated.is_none() {
            return Err(RegistryError::Conflict);
        }
        Ok(())
    }

    pub async fn state(&self, id: &ObjectId) -> Result<Option<ObjectState>, RegistryError> {
        let row = sqlx::query("SELECT state FROM storage_objects WHERE id=$1::uuid")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(sql_error)?;
        row.map(|value| {
            let state: String = value.try_get("state").map_err(sql_error)?;
            match state.as_str() {
                "pending" => Ok(ObjectState::Pending),
                "ready" => Ok(ObjectState::Ready),
                "attached" => Ok(ObjectState::Attached),
                "deleting" => Ok(ObjectState::Deleting),
                _ => Err(RegistryError::Unavailable),
            }
        })
        .transpose()
    }
}
