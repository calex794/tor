//! F-05 / C-15: explicit completion of a pinned encrypted paid V1.
//! A cursor, grant, S3 download or browser-supplied end is not an event.
use super::{checked_place, PgReadingSync, ReadingPlace, Row, SyncError};
use tor_contracts::reader_manifest::ReaderVersionManifestV1;

impl PgReadingSync {
    /// Scoped read-only capability for H. Old S3 rows without the trusted
    /// G manifest stay ineligible; the absence of rights looks like 404.
    pub async fn purchased_s3_completion_state(
        &self,
        actor: i64,
        purchase_id: i64,
        chapter_id: i64,
        version_id: i64,
    ) -> Result<(bool, bool), SyncError> {
        if actor <= 0 || purchase_id <= 0 || chapter_id <= 0 || version_id <= 0 {
            return Err(SyncError::Invalid);
        }
        let row = sqlx::query(
            "SELECT m.version_id IS NOT NULL AS can_complete,
                    EXISTS(
                        SELECT 1 FROM reader_chapter_completions c
                        WHERE c.user_id=$1 AND c.book_id=chapter.book_id
                          AND c.chapter_id=$3 AND c.first_version_id=$4
                          AND c.purchase_id=$2
                    ) AS completed
             FROM chapter_purchase_facts purchase
             JOIN chapter_versions edition ON edition.id=purchase.version_id
               AND edition.chapter_id=purchase.chapter_id
             JOIN chapters chapter ON chapter.id=purchase.chapter_id
             JOIN storage_objects o ON o.id=edition.storage_object_id
             LEFT JOIN chapter_reader_manifests m ON m.chapter_id=chapter.id
               AND m.version_id=edition.id AND m.storage_object_id=o.id
               AND m.parser_schema=1 AND m.content_sha256=o.plaintext_sha256
             WHERE purchase.user_id=$1 AND purchase.id=$2
               AND purchase.chapter_id=$3 AND purchase.version_id=$4
               AND tor_purchase_version_readable($1,$2,$3,$4)
               AND o.object_kind='chapter' AND o.codec='gzip'
               AND o.state='attached'",
        )
        .bind(actor)
        .bind(purchase_id)
        .bind(chapter_id)
        .bind(version_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| SyncError::Unavailable)?
        .ok_or(SyncError::NotFound)?;
        Ok((
            row.try_get("can_complete")
                .map_err(|_| SyncError::Unavailable)?,
            row.try_get("completed")
                .map_err(|_| SyncError::Unavailable)?,
        ))
    }

    /// Validate B's immutable manifest and D's current pinned right
    /// under the same transaction as CAS, one C completion and the F outbox.
    /// Returns (C event id, newly completed). No plaintext is read or emitted.
    pub async fn complete_purchased_s3(
        &self,
        actor: i64,
        purchase_id: i64,
        chapter_id: i64,
        version_id: i64,
        place: ReadingPlace,
        expected_revision: i64,
        plaintext_sha256: &str,
    ) -> Result<(String, bool), SyncError> {
        if actor <= 0 || purchase_id <= 0 || chapter_id <= 0 || version_id <= 0
            || plaintext_sha256.len() != 64
            || !plaintext_sha256.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(SyncError::Invalid);
        }
        checked_place(place, expected_revision)?;
        let mut tx = self.pool.begin().await.map_err(|_| SyncError::Unavailable)?;
        // Block the D purchase before a concurrent refund can insert its FK.
        // Hold B moderation and the immutable attached G object through commit.
        let row = sqlx::query(
            "SELECT chapter.book_id, m.block_utf16_lengths,
                    m.last_readable_block_index, m.last_readable_char_offset,
                    encode(m.content_sha256,'hex') AS plaintext_hash
             FROM chapter_purchase_facts purchase
             JOIN chapters chapter ON chapter.id=purchase.chapter_id
             JOIN books book ON book.id=chapter.book_id
             JOIN chapter_versions edition ON edition.id=purchase.version_id
               AND edition.chapter_id=purchase.chapter_id
             JOIN storage_objects o ON o.id=edition.storage_object_id
             JOIN chapter_reader_manifests m ON m.chapter_id=chapter.id
               AND m.version_id=edition.id AND m.storage_object_id=o.id
             WHERE purchase.user_id=$1 AND purchase.id=$2
               AND purchase.chapter_id=$3 AND purchase.version_id=$4
               AND o.object_kind='chapter' AND o.codec='gzip'
               AND o.state='attached'
               AND m.parser_schema=1 AND m.content_sha256=o.plaintext_sha256
               AND o.object_aad=convert_to(
                 concat('tor.encrypted.chapter.v1',chr(10),$3::text,chr(10),
                   $4::text,chr(10),o.id::text,chr(10),
                   encode(o.plaintext_sha256,'hex'),chr(10)), 'UTF8')
             FOR UPDATE OF purchase FOR SHARE OF chapter,book,edition,o",
        )
        .bind(actor)
        .bind(purchase_id)
        .bind(chapter_id)
        .bind(version_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| SyncError::Unavailable)?
        .ok_or(SyncError::NotFound)?;
        let permitted: bool = sqlx::query_scalar(
            "SELECT tor_purchase_version_readable($1,$2,$3,$4)",
        )
        .bind(actor)
        .bind(purchase_id)
        .bind(chapter_id)
        .bind(version_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| SyncError::Unavailable)?;
        if !permitted {
            return Err(SyncError::NotFound);
        }
        let book_id: i64 = row.try_get("book_id").map_err(|_| SyncError::Unavailable)?;
        let manifest = ReaderVersionManifestV1 {
            block_utf16_lengths: row
                .try_get("block_utf16_lengths")
                .map_err(|_| SyncError::Unavailable)?,
            last_readable_block_index: row
                .try_get("last_readable_block_index")
                .map_err(|_| SyncError::Unavailable)?,
            last_readable_char_offset: row
                .try_get("last_readable_char_offset")
                .map_err(|_| SyncError::Unavailable)?,
        };
        let hash: String = row.try_get("plaintext_hash")
            .map_err(|_| SyncError::Unavailable)?;
        if !manifest.is_completed_place(place.block_index, place.char_offset)
            || hash != plaintext_sha256
        {
            return Err(SyncError::Invalid);
        }

        // An already committed fact is replay-safe, including requests from
        // a second device. Never create another event or another revision.
        if let Some(existing) = sqlx::query_scalar::<_, String>(
            "SELECT source_event_id::text FROM reader_chapter_completions
             WHERE user_id=$1 AND book_id=$2 AND chapter_id=$3",
        )
        .bind(actor)
        .bind(book_id)
        .bind(chapter_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| SyncError::Unavailable)? {
            return Ok((existing, false));
        }

        let receipt = sqlx::query(
            "INSERT INTO reader_purchased_positions AS saved
                 (user_id,purchase_id,chapter_id,version_id,
                  block_index,char_offset,revision)
             SELECT $1,$2,$3,$4,$5,$6,1
             WHERE tor_purchase_version_readable($1,$2,$3,$4)
               AND ($7=0 OR EXISTS(
                 SELECT 1 FROM reader_purchased_positions old
                 WHERE old.user_id=$1 AND old.purchase_id=$2 AND old.revision=$7
               ))
             ON CONFLICT(user_id,purchase_id) DO UPDATE SET
                 block_index=EXCLUDED.block_index,
                 char_offset=EXCLUDED.char_offset,
                 revision=saved.revision+1,updated_at=now()
             WHERE saved.revision=$7
               AND (saved.block_index < EXCLUDED.block_index
                 OR (saved.block_index=EXCLUDED.block_index
                     AND saved.char_offset <= EXCLUDED.char_offset))
             RETURNING revision",
        )
        .bind(actor)
        .bind(purchase_id)
        .bind(chapter_id)
        .bind(version_id)
        .bind(place.block_index)
        .bind(place.char_offset)
        .bind(expected_revision)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| SyncError::Unavailable)?;
        if receipt.is_none() {
            return Err(SyncError::Conflict);
        }

        let created = sqlx::query_scalar::<_, String>(
            "INSERT INTO reader_chapter_completions
                (user_id,book_id,chapter_id,first_version_id,purchase_id,source_event_id)
             VALUES($1,$2,$3,$4,$5,gen_random_uuid())
             ON CONFLICT(user_id,book_id,chapter_id) DO NOTHING
             RETURNING source_event_id::text",
        )
        .bind(actor)
        .bind(book_id)
        .bind(chapter_id)
        .bind(version_id)
        .bind(purchase_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| SyncError::Unavailable)?;
        let (event_id, is_new) = if let Some(event_id) = created {
            sqlx::query(
                "INSERT INTO outbox_events(
                    id,event_type,schema_version,aggregate_kind,aggregate_id,
                    dedupe_key,payload,required_consumers)
                 VALUES(
                    $1::uuid,'ChapterCompletedV1',1,'chapter',$4,
                    concat('chapter-completed:',$2,':',$4),
                    jsonb_build_object('user_id',$2,'book_id',$3,
                        'chapter_id',$4,'version_id',$5,'purchase_id',$6,
                        'authority','C-verified-purchased-position'),
                    ARRAY['ranking-eligibility-v1'])",
            )
            .bind(&event_id)
            .bind(actor)
            .bind(book_id)
            .bind(chapter_id)
            .bind(version_id)
            .bind(purchase_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| SyncError::Unavailable)?;
            (event_id, true)
        } else {
            let event_id = sqlx::query_scalar(
                "SELECT source_event_id::text FROM reader_chapter_completions
                 WHERE user_id=$1 AND book_id=$2 AND chapter_id=$3",
            )
            .bind(actor)
            .bind(book_id)
            .bind(chapter_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| SyncError::Unavailable)?;
            (event_id, false)
        };
        tx.commit().await.map_err(|_| SyncError::Unavailable)?;
        Ok((event_id, is_new))
    }
}
