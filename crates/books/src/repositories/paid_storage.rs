//! B S3-09/11: owner-only atomic verified paid edition attachment and publish.
//! No S3 call inside this transaction: G uploads/verifies before B commits.
//! No entitlement is created here; D remains the sole rights authority.
use crate::services::authoring::AuthorError;
use async_trait::async_trait;
use sqlx::{PgPool, Row};
use tor_contracts::paid_delivery::{
    ChapterVersionPort, ChapterVersionRef, ContentCodec, DeliveryError, EncryptedObjectKind,
    EncryptedObjectRef, ObjectId, ObjectRef,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaidPublication {
    /// Attach verified historical V1 without changing the reader pointer.
    KeepCurrent,
    /// B-owned compare-and-swap of the published pointer. None = first release.
    Publish { expected_current: Option<i64> },
}

fn map_write(error: sqlx::Error) -> AuthorError {
    if let sqlx::Error::Database(ref db) = error {
        if matches!(
            db.code().as_deref(),
            Some("23503" | "23505" | "23514")
        ) {
            return AuthorError::Conflict;
        }
    }
    AuthorError::Unavailable
}

#[derive(Clone)]
pub struct PgPaidStorage {
    pool: PgPool,
}

impl PgPaidStorage {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The only B-owned path from a verified G ready object to a paid version.
    /// Locks B book/chapter, checks actor, then locks/attaches the G object and
    /// moves publication CAS + release fact in one PostgreSQL transaction.
    /// A failed or stale publication leaves the object un-attached (rollback).
    pub async fn attach_verified_and_publish(
        &self,
        actor: i64,
        book_id: i64,
        version: ChapterVersionRef,
        object_id: &ObjectId,
        publication: PaidPublication,
    ) -> Result<(), AuthorError> {
        let chapter_id = version.chapter_id.get();
        let version_id = version.version_id.get();
        if actor <= 0 || book_id <= 0 {
            return Err(AuthorError::Invalid);
        }
        if let PaidPublication::Publish {
            expected_current: Some(expected),
        } = publication
        {
            if expected <= 0 || expected == version_id {
                return Err(AuthorError::Invalid);
            }
        }

        let mut tx = self.pool.begin().await.map_err(map_write)?;
        let book_visibility: Option<String> = sqlx::query_scalar(
            "SELECT visibility FROM books
             WHERE id=$1 AND owner_user_id=$2
               AND NOT admin_hold AND NOT is_deleted FOR UPDATE",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_write)?;
        match book_visibility.as_deref() {
            None => return Err(AuthorError::Missing),
            Some("public") => {}
            Some(_) => return Err(AuthorError::Conflict),
        }

        // Prevent a free chapter from being linked to paid-only ciphertext.
        let chapter_visibility: Option<String> = sqlx::query_scalar(
            "SELECT visibility FROM chapters
             WHERE id=$1 AND book_id=$2 AND access_level='paid'
               AND price_amount>0 AND NOT admin_hold AND NOT is_deleted
               AND (reader_released_at IS NULL OR reader_released_at<=now())
             FOR UPDATE",
        )
        .bind(chapter_id)
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_write)?;
        let Some(chapter_visibility) = chapter_visibility else {
            return Err(AuthorError::Missing);
        };

        // For historical V1, allow attachment without moving a newer V2.
        let existing: Option<Option<String>> = sqlx::query_scalar(
            "SELECT storage_object_id::text FROM chapter_versions
             WHERE id=$1 AND chapter_id=$2 FOR UPDATE",
        )
        .bind(version_id)
        .bind(chapter_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_write)?;
        let Some(previous_object) = existing else {
            return Err(AuthorError::Missing);
        };
        if previous_object.as_deref().is_some_and(|id| id != object_id.as_str()) {
            return Err(AuthorError::Conflict);
        }
        let current: Option<i64> = sqlx::query_scalar(
            "SELECT version_id FROM chapter_publications WHERE chapter_id=$1 FOR UPDATE",
        )
        .bind(chapter_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_write)?;

        if let PaidPublication::Publish { expected_current } = publication {
            // Exactly replayed publish is safe, but a stale caller cannot move
            // a newer published version backwards.
            if current != Some(version_id) && current != expected_current {
                return Err(AuthorError::Conflict);
            }
            if current == Some(version_id) && chapter_visibility != "public" {
                return Err(AuthorError::Conflict);
            }
        }

        if previous_object.is_none() {
            let attached: Option<String> = sqlx::query_scalar(
                "UPDATE storage_objects SET state='attached',attached_at=now(),
                    delete_after=NULL
                 WHERE id=$1::uuid AND state='ready' AND object_kind='chapter'
                 RETURNING id::text",
            )
            .bind(object_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_write)?;
            if attached.is_none() {
                return Err(AuthorError::Conflict);
            }

            // The B version trigger independently proves G's plaintext SHA +
            // version-specific AAD and rejects modifying any other B field.
            let linked: Option<i64> = sqlx::query_scalar(
                "UPDATE chapter_versions SET storage_object_id=$3::uuid
                 WHERE chapter_id=$1 AND id=$2 AND storage_object_id IS NULL
                 RETURNING id",
            )
            .bind(chapter_id)
            .bind(version_id)
            .bind(object_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_write)?;
            if linked != Some(version_id) {
                return Err(AuthorError::Conflict);
            }
        } else {
            let attached: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                     SELECT 1 FROM storage_objects
                     WHERE id=$1::uuid AND state='attached' AND object_kind='chapter'
                 )",
            )
            .bind(object_id.as_str())
            .fetch_one(&mut *tx)
            .await
            .map_err(map_write)?;
            if !attached {
                return Err(AuthorError::Conflict);
            }
        }

        if let PaidPublication::Publish { .. } = publication {
            if current != Some(version_id) {
                match current {
                    Some(old_version) => {
                        let changed = sqlx::query(
                            "UPDATE chapter_publications
                             SET version_id=$3,published_at=now()
                             WHERE chapter_id=$1 AND version_id=$2",
                        )
                        .bind(chapter_id)
                        .bind(old_version)
                        .bind(version_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(map_write)?;
                        if changed.rows_affected() != 1 {
                            return Err(AuthorError::Conflict);
                        }
                    }
                    None => {
                        sqlx::query(
                            "INSERT INTO chapter_publications(chapter_id,version_id)
                             VALUES($1,$2)",
                        )
                        .bind(chapter_id)
                        .bind(version_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(map_write)?;
                        sqlx::query(
                            "INSERT INTO chapter_first_release_events(chapter_id,version_id)
                             VALUES($1,$2) ON CONFLICT(chapter_id) DO NOTHING",
                        )
                        .bind(chapter_id)
                        .bind(version_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(map_write)?;
                    }
                }
                sqlx::query("UPDATE chapters SET visibility='public' WHERE id=$1")
                    .bind(chapter_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(map_write)?;
                sqlx::query(
                    "INSERT INTO chapter_encrypted_release_events
                       (chapter_id,version_id,storage_object_id)
                     VALUES($1,$2,$3::uuid) ON CONFLICT(chapter_id,version_id) DO NOTHING",
                )
                .bind(chapter_id)
                .bind(version_id)
                .bind(object_id.as_str())
                .execute(&mut *tx)
                .await
                .map_err(map_write)?;
            }
        }
        tx.commit().await.map_err(map_write)?;
        Ok(())
    }
}

/// B resolves historical versions only after D returns an Allowed pinned
/// edition. No query joins the current-publication pointer: V1 is retained
/// after a later V2, but hidden/held/removed content is never returned.
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
               AND c.visibility='public' AND NOT c.admin_hold AND NOT c.is_deleted
               AND b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
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
        let uuid: String = row.try_get("object_id").map_err(|_| DeliveryError::Unavailable)?;
        let size: i64 = row.try_get("size_bytes").map_err(|_| DeliveryError::Unavailable)?;
        let digest: Vec<u8> = row.try_get("sha256_ciphertext").map_err(|_| DeliveryError::Unavailable)?;
        let nonce: Vec<u8> = row.try_get("object_nonce").map_err(|_| DeliveryError::Unavailable)?;
        let aad: Vec<u8> = row.try_get("object_aad").map_err(|_| DeliveryError::Unavailable)?;
        let kek_id: String = row.try_get("kek_id").map_err(|_| DeliveryError::Unavailable)?;
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
