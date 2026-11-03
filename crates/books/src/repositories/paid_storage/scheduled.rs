//! S058: a paid chapter may be presealed while remaining reader-invisible.
//! B activates only an exact G-attached version; entitlement stays with D.
use super::{map_write, PgPaidStorage, PreparedPaidChapter};
use crate::services::authoring::{AuthorError, SaveChapter};
use sqlx::{Postgres, Transaction};

impl PgPaidStorage {
    /// Reserve the edition and delayed mode in the same idempotent author
    /// transaction. A retry in immediate mode conflicts before any S3 write.
    pub async fn prepare_scheduled_paid_chapter(
        &self,
        actor: i64,
        request: &SaveChapter,
    ) -> Result<PreparedPaidChapter, AuthorError> {
        self.prepare_new_paid_chapter_with_mode(actor, request, true)
            .await
    }

    /// Fail closed: delayed paid creation needs an enabled book-level cadence.
    /// Cancelling the cadence after this check can only leave a private draft.
    pub async fn require_active_book_schedule(
        &self,
        actor: i64,
        book: i64,
    ) -> Result<(), AuthorError> {
        if actor <= 0 || book <= 0 {
            return Err(AuthorError::Invalid);
        }
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT b.id FROM books b
             JOIN book_delayed_publication_settings s ON s.book_id=b.id
             WHERE b.id=$1 AND b.owner_user_id=$2 AND b.visibility='public'
               AND NOT b.admin_hold AND NOT b.is_deleted AND s.enabled",
        )
        .bind(book)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_write)?;
        found.map(|_| ()).ok_or(AuthorError::Conflict)
    }

    /// An upload might finish after its slot was claimed. B may retry that
    /// *same* blocked occurrence once its exact immutable paid version is
    /// G-attached; no slot is skipped and no earlier access is granted.
    /// Lock order matches the worker: Book -> settings -> occurrence.
    pub async fn rearm_prepared_paid_frontier(
        &self,
        actor: i64,
        book: i64,
        chapter: i64,
        version: i64,
    ) -> Result<bool, AuthorError> {
        if actor <= 0 || book <= 0 || chapter <= 0 || version <= 0 {
            return Err(AuthorError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(map_write)?;
        let owned: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM books WHERE id=$1 AND owner_user_id=$2
               AND NOT admin_hold AND NOT is_deleted FOR UPDATE",
        )
        .bind(book)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_write)?;
        owned.ok_or(AuthorError::Missing)?;
        let enabled: Option<i64> = sqlx::query_scalar(
            "SELECT book_id FROM book_delayed_publication_settings
             WHERE book_id=$1 AND enabled FOR UPDATE",
        )
        .bind(book)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_write)?;
        if enabled.is_none() {
            return Ok(false);
        }
        let prepared: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM chapters c
                JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=$3
                JOIN storage_objects o ON o.id=v.storage_object_id
                JOIN chapter_reader_manifests m
                  ON m.chapter_id=c.id AND m.version_id=v.id
                 AND m.storage_object_id=o.id
                WHERE c.id=$2 AND c.book_id=$1 AND c.visibility='draft'
                  AND c.access_level='paid' AND c.price_amount>0
                  AND v.version_no=c.edit_revision
                  AND NOT c.admin_hold AND NOT c.is_deleted
                  AND o.state='attached' AND o.object_kind='chapter'
                  AND o.verified_at IS NOT NULL
                  AND NOT EXISTS (
                     SELECT 1 FROM chapter_publications p WHERE p.chapter_id=c.id
                  )
            )",
        )
        .bind(book)
        .bind(chapter)
        .bind(version)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_write)?;
        if !prepared {
            // Another worker may have committed this exact paid edition
            // between G attachment and this rearm attempt. That is a valid
            // successful release, not a reason to retry or publish again.
            let published: bool = sqlx::query_scalar(
                "SELECT EXISTS (
                    SELECT 1 FROM chapters c
                    JOIN chapter_publications p ON p.chapter_id=c.id
                    WHERE c.id=$2 AND c.book_id=$1 AND c.visibility='public'
                      AND c.access_level='paid' AND c.price_amount>0
                      AND p.version_id=$3
                )",
            )
            .bind(book)
            .bind(chapter)
            .bind(version)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_write)?;
            if published {
                return Ok(false);
            }
            return Err(AuthorError::Conflict);
        }
        let changed = sqlx::query(
            "UPDATE book_delayed_publication_occurrences o
             SET status='pending',last_error_code=NULL,completed_at=NULL
             FROM book_delayed_publication_settings s
             WHERE o.book_id=$1 AND o.chapter_id=$2
               AND s.book_id=o.book_id AND s.enabled
               AND o.schedule_revision=s.schedule_revision
               AND o.target_at=s.next_slot_at
               AND o.slot_index=s.next_slot_index
               AND o.status='blocked'
               AND o.last_error_code IN
                  ('paid_version_not_prepared',
                   'paid_frontier_needs_private_release',
                   'paid_frontier_not_supported')",
        )
        .bind(book)
        .bind(chapter)
        .execute(&mut *tx)
        .await
        .map_err(map_write)?
        .rows_affected();
        tx.commit().await.map_err(map_write)?;
        Ok(changed == 1)
    }

    /// Idempotent retry of the same author command never uploads a second
    /// ciphertext if its immutable edition has already been G-verified.
    pub async fn staged_paid_object_attached(
        &self,
        actor: i64,
        book: i64,
        chapter: i64,
        version: i64,
    ) -> Result<bool, AuthorError> {
        if actor <= 0 || book <= 0 || chapter <= 0 || version <= 0 {
            return Err(AuthorError::Invalid);
        }
        sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM books b
                JOIN chapters c ON c.book_id=b.id
                JOIN chapter_versions v ON v.chapter_id=c.id
                JOIN storage_objects o ON o.id=v.storage_object_id
                JOIN chapter_reader_manifests m
                  ON m.chapter_id=c.id AND m.version_id=v.id
                 AND m.storage_object_id=o.id
                WHERE b.id=$1 AND b.owner_user_id=$2
                  AND NOT b.admin_hold AND NOT b.is_deleted
                  AND c.id=$3 AND c.visibility='draft'
                  AND c.access_level='paid' AND c.price_amount>0
                  AND NOT c.admin_hold AND NOT c.is_deleted
                  AND v.id=$4 AND v.version_no=c.edit_revision
                  AND o.state='attached' AND o.object_kind='chapter'
                  AND o.verified_at IS NOT NULL
                  AND NOT EXISTS (
                    SELECT 1 FROM chapter_publications p WHERE p.chapter_id=c.id
                  )
             )",
        )
        .bind(book)
        .bind(actor)
        .bind(chapter)
        .bind(version)
        .fetch_one(&self.pool)
        .await
        .map_err(map_write)
    }
}

/// Caller holds Book -> settings -> occurrence -> Chapter locks.
/// No network I/O occurs here. One commit activates only an already
/// verified and manifest-backed paid version; the current price is retained.
pub(crate) async fn release_staged_paid_chapter_tx(
    tx: &mut Transaction<'_, Postgres>,
    actor: i64,
    book: i64,
    chapter: i64,
    expected_revision: i64,
    version: i64,
) -> Result<(), AuthorError> {
    if actor <= 0 || book <= 0 || chapter <= 0 || expected_revision <= 0 || version <= 0 {
        return Err(AuthorError::Invalid);
    }
    let owner: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM books
         WHERE id=$1 AND owner_user_id=$2 AND visibility='public'
           AND NOT admin_hold AND NOT is_deleted FOR UPDATE",
    )
    .bind(book)
    .bind(actor)
    .fetch_optional(&mut **tx)
    .await
    .map_err(map_write)?;
    owner.ok_or(AuthorError::Missing)?;

    // Lock exact edition and encrypted object after the Chapter. Any
    // missing metadata, future time gate or published pointer fails closed.
    let object: String = sqlx::query_scalar(
        "SELECT o.id::text FROM chapters c
         JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=$3
         JOIN storage_objects o ON o.id=v.storage_object_id
         JOIN chapter_reader_manifests m
           ON m.chapter_id=c.id AND m.version_id=v.id
          AND m.storage_object_id=o.id
         WHERE c.id=$1 AND c.book_id=$2
           AND c.visibility='draft' AND c.access_level='paid'
           AND c.price_amount>0 AND c.edit_revision=$4
           AND v.version_no=$4 AND NOT c.admin_hold AND NOT c.is_deleted
           AND (c.reader_released_at IS NULL
                OR c.reader_released_at<=clock_timestamp())
           AND o.state='attached' AND o.object_kind='chapter'
           AND o.verified_at IS NOT NULL
           AND NOT EXISTS (
              SELECT 1 FROM chapter_publications p WHERE p.chapter_id=c.id
           )
         FOR UPDATE OF c,v,o",
    )
    .bind(chapter)
    .bind(book)
    .bind(version)
    .bind(expected_revision)
    .fetch_optional(&mut **tx)
    .await
    .map_err(map_write)?
    .ok_or(AuthorError::Conflict)?;

    let inserted: Option<i64> = sqlx::query_scalar(
        "INSERT INTO chapter_publications(chapter_id,version_id)
         VALUES($1,$2) ON CONFLICT(chapter_id) DO NOTHING RETURNING chapter_id",
    )
    .bind(chapter)
    .bind(version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(map_write)?;
    if inserted != Some(chapter) {
        return Err(AuthorError::Conflict);
    }
    let changed = sqlx::query(
        "UPDATE chapters SET visibility='public'
         WHERE id=$1 AND edit_revision=$2 AND visibility='draft'
           AND access_level='paid' AND price_amount>0",
    )
    .bind(chapter)
    .bind(expected_revision)
    .execute(&mut **tx)
    .await
    .map_err(map_write)?
    .rows_affected();
    if changed != 1 {
        return Err(AuthorError::Conflict);
    }
    let event = sqlx::query(
        "INSERT INTO chapter_first_release_events
           (chapter_id,version_id,creator_user_id)
         VALUES($1,$2,$3) ON CONFLICT(chapter_id) DO NOTHING",
    )
    .bind(chapter)
    .bind(version)
    .bind(actor)
    .execute(&mut **tx)
    .await
    .map_err(map_write)?
    .rows_affected();
    if event != 1 {
        return Err(AuthorError::Conflict);
    }
    let encrypted = sqlx::query(
        "INSERT INTO chapter_encrypted_release_events
           (chapter_id,version_id,storage_object_id)
         VALUES($1,$2,$3::uuid)
         ON CONFLICT(chapter_id,version_id) DO NOTHING",
    )
    .bind(chapter)
    .bind(version)
    .bind(&object)
    .execute(&mut **tx)
    .await
    .map_err(map_write)?
    .rows_affected();
    if encrypted != 1 {
        return Err(AuthorError::Conflict);
    }
    Ok(())
}
