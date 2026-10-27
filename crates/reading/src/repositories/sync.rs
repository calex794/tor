//! Atomic PostgreSQL C-09 revision checks. No progress record is an entitlement.
#![forbid(unsafe_code)]

use crate::services::{
    progress::ReadingPlace,
    sync::{checked_place, StoredProgress, SyncError, SyncReceipt},
};
use sqlx::{postgres::PgRow, PgPool, Row};

#[derive(Clone)]
pub struct PgReadingSync {
    pool: PgPool,
}

impl PgReadingSync {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Existing free cursor (possibly on a different chapter/version of the book).
    /// Only a still-public requested chapter may expose a revision.
    pub async fn free_state(
        &self,
        actor: i64,
        chapter_id: i64,
    ) -> Result<Option<StoredProgress>, SyncError> {
        if actor <= 0 || chapter_id <= 0 {
            return Err(SyncError::Invalid);
        }
        let row = sqlx::query(
            "SELECT saved.chapter_id, saved.version_id, saved.block_index, \
                    saved.char_offset, saved.revision, \
                    to_char(saved.updated_at AT TIME ZONE 'UTC', \
                        'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS updated_at \
             FROM reader_book_positions AS saved \
             JOIN chapters AS chapter ON chapter.book_id = saved.book_id AND chapter.id = $2 \
             JOIN books AS book ON book.id = chapter.book_id \
             JOIN chapter_publications AS published ON published.chapter_id = chapter.id \
             WHERE saved.user_id = $1 AND book.visibility = 'public' \
               AND NOT book.admin_hold AND NOT book.is_deleted \
               AND chapter.visibility = 'public' AND chapter.access_level = 'free' \
               AND NOT chapter.admin_hold AND NOT chapter.is_deleted \
               AND (chapter.reader_released_at IS NULL OR chapter.reader_released_at <= now())",
        )
        .bind(actor)
        .bind(chapter_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| SyncError::Unavailable)?;
        row.map(parse_stored).transpose()
    }

    /// C-authoritative completion receipt, actor-scoped to the read chapter.
    /// Used by H to show a completed state and to nudge F immediately;
    /// background outbox delivery remains the durable fallback.
    pub async fn completion_event_id(
        &self,
        actor: i64,
        chapter_id: i64,
    ) -> Result<Option<String>, SyncError> {
        if actor <= 0 || chapter_id <= 0 {
            return Err(SyncError::Invalid);
        }
        sqlx::query_scalar(
            "SELECT source_event_id::text FROM reader_chapter_completions
             WHERE user_id=$1 AND chapter_id=$2",
        )
        .bind(actor)
        .bind(chapter_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| SyncError::Unavailable)
    }

    /// CAS: new=revision 0; existing=matching revision. No overwrite on a
    /// delayed request, backwards cursor, unpublished version or extra device.
    pub async fn save_free(
        &self,
        actor: i64,
        chapter_id: i64,
        version_id: i64,
        place: ReadingPlace,
        expected_revision: i64,
    ) -> Result<SyncReceipt, SyncError> {
        if actor <= 0 || chapter_id <= 0 || version_id <= 0 {
            return Err(SyncError::Invalid);
        }
        checked_place(place, expected_revision)?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| SyncError::Unavailable)?;
        // Lock exactly the published free version and its access gate until
        // both progress and completion/outbox have committed.
        let edition = sqlx::query(
            "SELECT chapter.book_id, edition.body_markdown
             FROM chapters AS chapter
             JOIN books AS book ON book.id=chapter.book_id
             JOIN chapter_publications AS publication ON publication.chapter_id=chapter.id
             JOIN chapter_versions AS edition
               ON edition.id=publication.version_id AND edition.chapter_id=chapter.id
             WHERE chapter.id=$1 AND publication.version_id=$2
               AND book.visibility='public' AND NOT book.admin_hold AND NOT book.is_deleted
               AND chapter.visibility='public' AND chapter.access_level='free'
               AND NOT chapter.admin_hold AND NOT chapter.is_deleted
               AND (chapter.reader_released_at IS NULL OR chapter.reader_released_at <= now())
             FOR SHARE OF chapter,book,publication,edition",
        )
        .bind(chapter_id)
        .bind(version_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| SyncError::Unavailable)?
        .ok_or(SyncError::Conflict)?;
        let book_id: i64 = edition
            .try_get("book_id")
            .map_err(|_| SyncError::Unavailable)?;
        let body: String = edition
            .try_get("body_markdown")
            .map_err(|_| SyncError::Unavailable)?;
        if !crate::domain::renderer::valid_reader_length(&body) {
            return Err(SyncError::Invalid);
        }
        let blocks = crate::services::progress::reader_blocks(&body);
        if crate::services::progress::valid_text_place(place, &blocks).is_err() {
            return Err(SyncError::Invalid);
        }
        let finished = crate::services::progress::is_completed_place(place, &blocks);
        let row = sqlx::query(
            "INSERT INTO reader_book_positions AS saved \
               (user_id, book_id, chapter_id, chapter_no, chapter_revision, \
                version_id, block_index, char_offset, revision) \
             SELECT $1, chapter.book_id, chapter.id, chapter.chapter_no, \
                    chapter.edit_revision, $3, $4, $5, 1 \
             FROM chapters AS chapter \
             JOIN books AS book ON book.id = chapter.book_id \
             JOIN chapter_publications AS published ON published.chapter_id = chapter.id \
             JOIN chapter_versions AS edition ON edition.id = published.version_id \
                 AND edition.chapter_id = chapter.id \
             WHERE chapter.id = $2 AND published.version_id = $3 \
               AND book.visibility = 'public' AND NOT book.admin_hold AND NOT book.is_deleted \
               AND chapter.visibility = 'public' AND chapter.access_level = 'free' \
               AND NOT chapter.admin_hold AND NOT chapter.is_deleted \
               AND (chapter.reader_released_at IS NULL OR chapter.reader_released_at <= now()) \
               AND ($6 = 0 OR EXISTS ( \
                  SELECT 1 FROM reader_book_positions AS old \
                  WHERE old.user_id = $1 AND old.book_id = chapter.book_id \
                    AND old.revision = $6 \
               )) \
             ON CONFLICT (user_id, book_id) DO UPDATE SET \
               chapter_id = EXCLUDED.chapter_id, \
               chapter_no = EXCLUDED.chapter_no, \
               chapter_revision = EXCLUDED.chapter_revision, \
               version_id = EXCLUDED.version_id, \
               block_index = EXCLUDED.block_index, \
               char_offset = EXCLUDED.char_offset, \
               revision = saved.revision + 1, updated_at = now() \
             WHERE saved.revision = $6 AND ( \
               saved.chapter_no < EXCLUDED.chapter_no \
               OR (saved.chapter_no = EXCLUDED.chapter_no AND ( \
                   saved.chapter_revision < EXCLUDED.chapter_revision \
                   OR (saved.chapter_revision = EXCLUDED.chapter_revision AND ( \
                       saved.version_id <> EXCLUDED.version_id \
                       OR saved.block_index < EXCLUDED.block_index \
                       OR (saved.block_index = EXCLUDED.block_index \
                           AND saved.char_offset < EXCLUDED.char_offset) \
                   )) \
               )) \
             ) \
             RETURNING revision, \
                to_char(updated_at AT TIME ZONE 'UTC', \
                    'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS updated_at",
        )
        .bind(actor)
        .bind(chapter_id)
        .bind(version_id)
        .bind(place.block_index)
        .bind(place.char_offset)
        .bind(expected_revision)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| SyncError::Unavailable)?;
        let receipt = row
            .map(parse_receipt)
            .transpose()?
            .ok_or(SyncError::Conflict)?;
        if finished {
            // One successful CAS + one unique C completion + one durable
            // outbox event. A V2 replay or a second device cannot duplicate it.
            sqlx::query(
                "WITH committed AS (
                    INSERT INTO reader_chapter_completions
                        (user_id,book_id,chapter_id,first_version_id,source_event_id)
                    VALUES($1,$2,$3,$4,gen_random_uuid())
                    ON CONFLICT(user_id,book_id,chapter_id) DO NOTHING
                    RETURNING source_event_id
                )
                INSERT INTO outbox_events(
                    id,event_type,schema_version,aggregate_kind,aggregate_id,
                    dedupe_key,payload,required_consumers
                )
                SELECT source_event_id,'ChapterCompletedV1',1,'chapter',$3,
                    concat('chapter-completed:', $1, ':', $3),
                    jsonb_build_object('user_id',$1,'book_id',$2,
                        'chapter_id',$3,'version_id',$4,
                        'authority','C-verified-free-position'),
                    ARRAY['ranking-eligibility-v1']
                FROM committed",
            )
            .bind(actor)
            .bind(book_id)
            .bind(chapter_id)
            .bind(version_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| SyncError::Unavailable)?;
        }
        tx.commit().await.map_err(|_| SyncError::Unavailable)?;
        Ok(receipt)
    }

    /// D-owned permission function, including immutable pinned edition and
    /// revocation. Never accept a purchase_id as proof by itself.
    pub async fn purchased_allowed(
        &self,
        actor: i64,
        purchase_id: i64,
        chapter_id: i64,
        version_id: i64,
    ) -> Result<bool, SyncError> {
        if actor <= 0 || purchase_id <= 0 || chapter_id <= 0 || version_id <= 0 {
            return Err(SyncError::Invalid);
        }
        sqlx::query_scalar("SELECT tor_purchase_version_readable($1, $2, $3, $4)")
            .bind(actor)
            .bind(purchase_id)
            .bind(chapter_id)
            .bind(version_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| SyncError::Unavailable)
    }

    /// Paid plaintext is used only internally to check a UTF-16 anchor after
    /// a real entitlement check. This repository never returns it over HTTP.
    pub async fn purchased_body_for_validation(
        &self,
        actor: i64,
        purchase_id: i64,
        chapter_id: i64,
        version_id: i64,
    ) -> Result<Option<String>, SyncError> {
        let body: Option<String> = sqlx::query_scalar(
            "SELECT edition.body_markdown FROM chapter_versions AS edition \
             WHERE edition.chapter_id = $3 AND edition.id = $4 \
               AND tor_purchase_version_readable($1, $2, $3, $4)",
        )
        .bind(actor)
        .bind(purchase_id)
        .bind(chapter_id)
        .bind(version_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| SyncError::Unavailable)?;
        Ok(body)
    }

    pub async fn purchased_state(
        &self,
        actor: i64,
        purchase_id: i64,
        chapter_id: i64,
        version_id: i64,
    ) -> Result<Option<StoredProgress>, SyncError> {
        if !self
            .purchased_allowed(actor, purchase_id, chapter_id, version_id)
            .await?
        {
            return Err(SyncError::NotFound);
        }
        let row = sqlx::query(
            "SELECT saved.chapter_id, saved.version_id, saved.block_index, \
                    saved.char_offset, saved.revision, \
                    to_char(saved.updated_at AT TIME ZONE 'UTC', \
                        'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS updated_at \
             FROM reader_purchased_positions AS saved \
             WHERE saved.user_id = $1 AND saved.purchase_id = $2 \
               AND saved.chapter_id = $3 AND saved.version_id = $4 \
               AND tor_purchase_version_readable($1, $2, $3, $4)",
        )
        .bind(actor)
        .bind(purchase_id)
        .bind(chapter_id)
        .bind(version_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| SyncError::Unavailable)?;
        row.map(parse_stored).transpose()
    }

    pub async fn save_purchased(
        &self,
        actor: i64,
        purchase_id: i64,
        chapter_id: i64,
        version_id: i64,
        place: ReadingPlace,
        expected_revision: i64,
    ) -> Result<SyncReceipt, SyncError> {
        if actor <= 0 || purchase_id <= 0 || chapter_id <= 0 || version_id <= 0 {
            return Err(SyncError::Invalid);
        }
        checked_place(place, expected_revision)?;
        let row = sqlx::query(
            "INSERT INTO reader_purchased_positions AS saved \
              (user_id, purchase_id, chapter_id, version_id, \
               block_index, char_offset, revision) \
             SELECT $1, $2, $3, $4, $5, $6, 1 \
             WHERE tor_purchase_version_readable($1, $2, $3, $4) \
               AND ($7 = 0 OR EXISTS ( \
                  SELECT 1 FROM reader_purchased_positions AS old \
                  WHERE old.user_id = $1 AND old.purchase_id = $2 \
                    AND old.revision = $7 \
               )) \
             ON CONFLICT (user_id, purchase_id) DO UPDATE SET \
               block_index = EXCLUDED.block_index, \
               char_offset = EXCLUDED.char_offset, \
               revision = saved.revision + 1, updated_at = now() \
             WHERE saved.revision = $7 \
               AND (saved.block_index < EXCLUDED.block_index \
                 OR (saved.block_index = EXCLUDED.block_index \
                     AND saved.char_offset < EXCLUDED.char_offset)) \
             RETURNING revision, \
                 to_char(updated_at AT TIME ZONE 'UTC', \
                     'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS updated_at",
        )
        .bind(actor)
        .bind(purchase_id)
        .bind(chapter_id)
        .bind(version_id)
        .bind(place.block_index)
        .bind(place.char_offset)
        .bind(expected_revision)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| SyncError::Unavailable)?;
        if let Some(row) = row {
            return parse_receipt(row);
        }
        if !self
            .purchased_allowed(actor, purchase_id, chapter_id, version_id)
            .await?
        {
            return Err(SyncError::NotFound);
        }
        Err(SyncError::Conflict)
    }
}

fn parse_receipt(row: PgRow) -> Result<SyncReceipt, SyncError> {
    Ok(SyncReceipt {
        revision: row
            .try_get("revision")
            .map_err(|_| SyncError::Unavailable)?,
        updated_at: row
            .try_get("updated_at")
            .map_err(|_| SyncError::Unavailable)?,
    })
}

fn parse_stored(row: PgRow) -> Result<StoredProgress, SyncError> {
    Ok(StoredProgress {
        chapter_id: row
            .try_get("chapter_id")
            .map_err(|_| SyncError::Unavailable)?,
        version_id: row
            .try_get("version_id")
            .map_err(|_| SyncError::Unavailable)?,
        place: ReadingPlace {
            block_index: row
                .try_get("block_index")
                .map_err(|_| SyncError::Unavailable)?,
            char_offset: row
                .try_get("char_offset")
                .map_err(|_| SyncError::Unavailable)?,
        },
        receipt: parse_receipt(row)?,
    })
}
