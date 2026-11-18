//! FND-00: owner-only source recovery for a published, bodyless paid edition.
//! B owns the published version; G reads ciphertext only after live PG owner
//! checks and never uses a client-supplied bucket, object key or DEK.
use super::{PgStorageRegistry, RegistryError};
use crate::{
    domain::provider::{valid_ciphertext_size, valid_private_bucket},
    services::{
        crypto::{open_registered_paid_chapter, ContentKeyring, RegisteredPaidChapter},
        provider::PrivateObjectStore,
    },
};
use sqlx::Row;
use tor_contracts::paid_delivery::{ChapterVersionRef, EncryptedObjectKind, ObjectId, ObjectRef};
use zeroize::Zeroizing;

impl PgStorageRegistry {
    /// Only an active book owner can fetch the exact current S3-only edition.
    /// The expected version fences B metadata against concurrent publication.
    /// Historical paid PG-body editions deliberately remain on the legacy
    /// author editor path until their independent migration is accepted.
    pub async fn owned_current_paid_markdown<S: PrivateObjectStore>(
        &self,
        actor: i64,
        book: i64,
        version: ChapterVersionRef,
        private_bucket: &str,
        store: &S,
        keyring: &ContentKeyring,
    ) -> Result<Option<Zeroizing<String>>, RegistryError> {
        if actor <= 0 || book <= 0 {
            return Ok(None);
        }
        let chapter = version.chapter_id.get();
        let expected_version = version.version_id.get();
        if chapter <= 0 || expected_version <= 0 {
            return Ok(None);
        }
        if !valid_private_bucket(private_bucket) {
            return Err(RegistryError::Invalid);
        }
        let row = sqlx::query(
            "SELECT v.id AS version_id,o.id::text AS object_id,
                    o.size_bytes,o.sha256_ciphertext,o.plaintext_sha256,
                    o.object_nonce,o.object_aad,o.encrypted_dek,
                    o.wrap_nonce,o.kek_id
             FROM books b
             JOIN users owner ON owner.id=b.owner_user_id
                AND owner.account_state='active'
             JOIN chapters c ON c.book_id=b.id
             JOIN chapter_publications p ON p.chapter_id=c.id
             JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=p.version_id
             JOIN chapter_encrypted_release_events e
                ON e.chapter_id=c.id AND e.version_id=v.id
               AND e.storage_object_id=v.storage_object_id
             JOIN storage_objects o ON o.id=e.storage_object_id
             JOIN chapter_reader_manifests m
                ON m.chapter_id=c.id AND m.version_id=v.id
               AND m.storage_object_id=o.id
               AND m.content_sha256=o.plaintext_sha256
             WHERE b.id=$1 AND b.owner_user_id=$2
               AND c.id=$3 AND o.bucket=$4 AND v.id=$5
               AND b.visibility='public' AND b.visibility_status='active' AND c.visibility='public'
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND NOT c.admin_hold AND NOT c.is_deleted
               AND c.access_level='paid' AND c.price_amount>0
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
               AND v.body_markdown IS NULL
               AND v.content_sha256=o.plaintext_sha256
               AND o.state='attached' AND o.object_kind='chapter'
               AND o.codec='gzip' AND o.verified_at IS NOT NULL
               AND o.object_key='private/chapter-versions/' || o.id::text || '.enc'",
        )
        .bind(book)
        .bind(actor)
        .bind(chapter)
        .bind(private_bucket)
        .bind(expected_version)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| RegistryError::Unavailable)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let actual_version: i64 = row
            .try_get("version_id")
            .map_err(|_| RegistryError::Unavailable)?;
        if actual_version != expected_version {
            return Err(RegistryError::Unavailable);
        }
        let object_id: String = row
            .try_get("object_id")
            .map_err(|_| RegistryError::Unavailable)?;
        let object_id = ObjectId::parse(&object_id).map_err(|_| RegistryError::Unavailable)?;
        let size: i64 = row
            .try_get("size_bytes")
            .map_err(|_| RegistryError::Unavailable)?;
        let digest: Vec<u8> = row
            .try_get("sha256_ciphertext")
            .map_err(|_| RegistryError::Unavailable)?;
        let ciphertext_sha256: [u8; 32] =
            digest.try_into().map_err(|_| RegistryError::Unavailable)?;
        let plain_digest: Vec<u8> = row
            .try_get("plaintext_sha256")
            .map_err(|_| RegistryError::Unavailable)?;
        let plaintext_sha256: [u8; 32] = plain_digest
            .try_into()
            .map_err(|_| RegistryError::Unavailable)?;
        let nonce: Vec<u8> = row
            .try_get("object_nonce")
            .map_err(|_| RegistryError::Unavailable)?;
        let nonce: [u8; 12] = nonce.try_into().map_err(|_| RegistryError::Unavailable)?;
        let wrapped_dek: Vec<u8> = row
            .try_get("encrypted_dek")
            .map_err(|_| RegistryError::Unavailable)?;
        let wrap_nonce: Vec<u8> = row
            .try_get("wrap_nonce")
            .map_err(|_| RegistryError::Unavailable)?;
        let wrap_nonce: [u8; 12] = wrap_nonce
            .try_into()
            .map_err(|_| RegistryError::Unavailable)?;
        let aad: Vec<u8> = row
            .try_get("object_aad")
            .map_err(|_| RegistryError::Unavailable)?;
        let kek_id: String = row
            .try_get("kek_id")
            .map_err(|_| RegistryError::Unavailable)?;
        let object = ObjectRef::new(
            object_id.clone(),
            EncryptedObjectKind::Chapter,
            ciphertext_sha256,
            size.try_into().map_err(|_| RegistryError::Unavailable)?,
        )
        .map_err(|_| RegistryError::Unavailable)?;
        if !valid_ciphertext_size(&object) {
            return Err(RegistryError::Unavailable);
        }

        // A freshly downloaded object must match the immutable PG receipt;
        // HEAD alone, a signed URL and a caller-supplied hash are insufficient.
        let ciphertext = store
            .get_verified(&object)
            .await
            .map_err(|_| RegistryError::Unavailable)?;
        let opened = open_registered_paid_chapter(
            RegisteredPaidChapter {
                version,
                object_id: &object_id,
                ciphertext_len: object.size_bytes,
                ciphertext_sha256: &ciphertext_sha256,
                plaintext_sha256: &plaintext_sha256,
                object_nonce: &nonce,
                aad: &aad,
                wrapped_dek: &wrapped_dek,
                wrap_nonce: &wrap_nonce,
                kek_id: &kek_id,
            },
            &ciphertext,
            keyring,
        )
        .map_err(|_| RegistryError::Unavailable)?;

        // Ownership/holds may change during S3 I/O; do not return source from
        // a stale authorization decision.
        let still_owner: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM books b
                JOIN users owner ON owner.id=b.owner_user_id
                    AND owner.account_state='active'
                JOIN chapters c ON c.book_id=b.id
                JOIN chapter_publications p ON p.chapter_id=c.id
                JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=p.version_id
                JOIN storage_objects o ON o.id=v.storage_object_id
                WHERE b.id=$1 AND b.owner_user_id=$2 AND c.id=$3 AND v.id=$4
                  AND b.visibility='public' AND b.visibility_status='active' AND c.visibility='public'
                  AND NOT b.admin_hold AND NOT b.is_deleted
                  AND NOT c.admin_hold AND NOT c.is_deleted
                  AND c.access_level='paid' AND c.price_amount>0
                  AND v.body_markdown IS NULL AND v.storage_object_id=$5::uuid
                  AND o.state='attached' AND o.object_kind='chapter' AND o.bucket=$6
                  AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
             )",
        )
        .bind(book)
        .bind(actor)
        .bind(chapter)
        .bind(version.version_id.get())
        .bind(object_id.as_str())
        .bind(private_bucket)
        .fetch_one(&self.pool)
        .await
        .map_err(|_| RegistryError::Unavailable)?;
        if !still_owner {
            return Err(RegistryError::Missing);
        }
        Ok(Some(opened))
    }
}
