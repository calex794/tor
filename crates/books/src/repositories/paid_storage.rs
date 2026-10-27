//! B S3-09/11: owner-only atomic verified paid edition attachment and publish.
//! No S3 call inside this transaction: G uploads/verifies before B commits.
//! No entitlement is created here; D remains the sole rights authority.
use crate::services::authoring::{valid_command_key, AuthorError, ChapterAccess, SaveChapter};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use tor_contracts::paid_delivery::{
    ChapterVersionPort, ChapterVersionRef, ContentCodec, DeliveryError, EncryptedObjectKind,
    EncryptedObjectRef, ObjectId, ObjectRef,
};
use tor_contracts::reader_manifest::{ReaderVersionManifestV1, READER_PARSER_SCHEMA_V1};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaidPublication {
    /// Attach verified historical V1 without changing the reader pointer.
    KeepCurrent,
    /// B-owned compare-and-swap of the published pointer. None = first release.
    Publish { expected_current: Option<i64> },
}

/// Durable B-owned draft, not visible until G verifies the ciphertext
/// and the B attachment transaction commits. Identical command replays
/// reuse the same chapter/version rather than creating another edition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreparedPaidChapter {
    pub chapter_id: i64,
    pub version_id: i64,
    pub published: bool,
}

fn map_write(error: sqlx::Error) -> AuthorError {
    if let sqlx::Error::Database(ref db) = error {
        if matches!(db.code().as_deref(), Some("23503" | "23505" | "23514")) {
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

    /// First half of author Save: reserve a paid chapter/version, still
    /// hidden from readers. S3 encryption MUST happen only after this commit
    /// because AAD needs the server-issued chapter and version identifiers.
    /// Idempotency is owned by the same B command table as free authoring.
    pub async fn prepare_new_paid_chapter(
        &self,
        actor: i64,
        request: &SaveChapter,
    ) -> Result<PreparedPaidChapter, AuthorError> {
        if actor <= 0
            || request.book_id <= 0
            || request.chapter_id.is_some()
            || request.expected_revision != 0
            || request.number <= 0
            || request.access != ChapterAccess::Paid
            || request.price_amount <= 0
            || !valid_command_key(&request.command_key)
            || !(1..=200).contains(&request.title.chars().count())
            || request.title.chars().any(char::is_control)
            || !(1..=50_000).contains(&request.body.chars().count())
            || request.body.contains(['\\0', '\\r'])
        {
            return Err(AuthorError::Invalid);
        }
        let mut hasher = Sha256::new();
        for field in [
            "new-paid-chapter-v1".to_owned(),
            request.book_id.to_string(),
            request.number.to_string(),
            request.title.clone(),
            request.body.clone(),
            request.access.as_str().to_owned(),
            request.price_amount.to_string(),
        ] {
            hasher.update((field.len() as u64).to_be_bytes());
            hasher.update(field.as_bytes());
        }
        let payload_hash: [u8; 32] = hasher.finalize().into();
        let content_hash: [u8; 32] = Sha256::digest(request.body.as_bytes()).into();
        let mut tx = self.pool.begin().await.map_err(map_write)?;
        // Lock owner book before chapter creation; a non-owner cannot infer
        // whether the book ID exists, and a draft book cannot publish a paid
        // chapter with a publicly routable chapter URL.
        let visibility: Option<String> = sqlx::query_scalar(
            "SELECT visibility FROM books
             WHERE id=$1 AND owner_user_id=$2 AND NOT admin_hold AND NOT is_deleted
             FOR UPDATE",
        )
        .bind(request.book_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_write)?;
        match visibility.as_deref() {
            None => return Err(AuthorError::Missing),
            Some("public") => {}
            Some(_) => return Err(AuthorError::Conflict),
        }
        let claimed: Option<i64> = sqlx::query_scalar(
            "INSERT INTO chapter_save_commands(user_id,command_key,payload_hash)
             VALUES($1,$2,$3) ON CONFLICT(user_id,command_key) DO NOTHING
             RETURNING user_id",
        )
        .bind(actor)
        .bind(&request.command_key)
        .bind(payload_hash.as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_write)?;
        if claimed.is_none() {
            let row = sqlx::query(
                "SELECT payload_hash,chapter_id,version_id
                 FROM chapter_save_commands
                 WHERE user_id=$1 AND command_key=$2 FOR UPDATE",
            )
            .bind(actor)
            .bind(&request.command_key)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_write)?;
            let prior: Vec<u8> = row.try_get("payload_hash").map_err(|_| AuthorError::Unavailable)?;
            if prior != payload_hash {
                return Err(AuthorError::Conflict);
            }
            let chapter_id: i64 = row
                .try_get::<Option<i64>, _>("chapter_id")
                .map_err(|_| AuthorError::Unavailable)?
                .ok_or(AuthorError::Conflict)?;
            let version_id: i64 = row
                .try_get::<Option<i64>, _>("version_id")
                .map_err(|_| AuthorError::Unavailable)?
                .ok_or(AuthorError::Conflict)?;
            let published: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM chapter_publications
                 WHERE chapter_id=$1 AND version_id=$2)",
            )
            .bind(chapter_id)
            .bind(version_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_write)?;
            return Ok(PreparedPaidChapter {
                chapter_id,
                version_id,
                published,
            });
        }

        // The chapter must not enter catalog/reader until B verifies an
        // attached G object. Failed S3 leaves only an owner-visible draft.
        let chapter_id: i64 = sqlx::query_scalar(
            "INSERT INTO chapters(
                 book_id,chapter_no,title,visibility,access_level,price_amount,edit_revision
             ) VALUES($1,$2,$3,'draft','paid',$4,1) RETURNING id",
        )
        .bind(request.book_id)
        .bind(request.number)
        .bind(&request.title)
        .bind(request.price_amount)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_write)?;
        let version_id: i64 = sqlx::query_scalar(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
             VALUES($1,1,$2,$3) RETURNING id",
        )
        .bind(chapter_id)
        .bind(&request.body)
        .bind(content_hash.as_slice())
        .fetch_one(&mut *tx)
        .await
        .map_err(map_write)?;
        sqlx::query(
            "UPDATE chapter_save_commands
             SET chapter_id=$3,version_id=$4,saved_revision=1
             WHERE user_id=$1 AND command_key=$2",
        )
        .bind(actor)
        .bind(&request.command_key)
        .bind(chapter_id)
        .bind(version_id)
        .execute(&mut *tx)
        .await
        .map_err(map_write)?;
        tx.commit().await.map_err(map_write)?;
        Ok(PreparedPaidChapter {
            chapter_id,
            version_id,
            published: false,
        })
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
        if previous_object
            .as_deref()
            .is_some_and(|id| id != object_id.as_str())
        {
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

        // B creates the immutable parser manifest in the same commit as
        // the already-verified object attachment. C can later validate a
        // paid cursor without fetching or decrypting the S3 ciphertext.
        // Never copy plaintext, signed URLs or user data to this table.
        let body: String = sqlx::query_scalar(
            "SELECT body_markdown FROM chapter_versions
             WHERE chapter_id=$1 AND id=$2",
        )
        .bind(chapter_id)
        .bind(version_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_write)?;
        let manifest = ReaderVersionManifestV1::from_markdown(&body).ok_or(AuthorError::Invalid)?;
        let normalized = body.replace("\r\n", "\n").replace('\r', "\n");
        let digest: [u8; 32] = Sha256::digest(normalized.as_bytes()).into();
        sqlx::query(
            "INSERT INTO chapter_reader_manifests (
                 chapter_id,version_id,storage_object_id,parser_schema,
                 content_sha256,block_utf16_lengths,
                 last_readable_block_index,last_readable_char_offset
             ) VALUES($1,$2,$3::uuid,$4,$5,$6,$7,$8)
             ON CONFLICT(chapter_id,version_id) DO NOTHING",
        )
        .bind(chapter_id)
        .bind(version_id)
        .bind(object_id.as_str())
        .bind(READER_PARSER_SCHEMA_V1)
        .bind(digest.as_slice())
        .bind(&manifest.block_utf16_lengths)
        .bind(manifest.last_readable_block_index)
        .bind(manifest.last_readable_char_offset)
        .execute(&mut *tx)
        .await
        .map_err(map_write)?;
        // An exact replay is idempotent; any disagreement on parser, object,
        // edition or offsets is a hard conflict rather than a silent update.
        let identical: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1 FROM chapter_reader_manifests
                 WHERE chapter_id=$1 AND version_id=$2
                   AND storage_object_id=$3::uuid AND parser_schema=$4
                   AND content_sha256=$5 AND block_utf16_lengths=$6
                   AND last_readable_block_index=$7
                   AND last_readable_char_offset=$8
             )",
        )
        .bind(chapter_id)
        .bind(version_id)
        .bind(object_id.as_str())
        .bind(READER_PARSER_SCHEMA_V1)
        .bind(digest.as_slice())
        .bind(&manifest.block_utf16_lengths)
        .bind(manifest.last_readable_block_index)
        .bind(manifest.last_readable_char_offset)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_write)?;
        if !identical {
            return Err(AuthorError::Conflict);
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
