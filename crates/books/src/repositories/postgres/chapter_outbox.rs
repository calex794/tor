//! B/G-06: write first-release facts and their durable notification envelope
//! in the same owning PostgreSQL transaction. No external delivery here.
use crate::services::authoring::AuthorError;
use sqlx::{Postgres, Transaction};

/// The immutable edition already has a first-release fact in the transaction.
/// Replayed commands cannot produce a second root event. Never store body text.
pub(super) async fn enqueue_chapter_first_release(
    tx: &mut Transaction<'_, Postgres>,
    book_id: i64,
    chapter_id: i64,
    version_id: i64,
) -> Result<(), AuthorError> {
    sqlx::query(
        "INSERT INTO outbox_events
           (event_type,schema_version,aggregate_kind,aggregate_id,
            dedupe_key,payload,required_consumers)
         VALUES('ChapterPublished',1,'chapter',$1,
                'chapter-published:' || $1::text || ':' || $2::text,
                jsonb_build_object(
                    'chapter_id',$1,'book_id',$3,'version_id',$2,
                    'published_at',clock_timestamp(),'first_release',true),
                ARRAY['chapter_release_fanout_v1']::text[])
         ON CONFLICT(dedupe_key) DO NOTHING",
    )
    .bind(chapter_id)
    .bind(version_id)
    .bind(book_id)
    .execute(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    Ok(())
}

/// First release of a draft book: insert each first-release fact, with its
/// creator, and its event in one bulk statement. Existing facts are untouched.
pub(super) async fn enqueue_book_first_releases(
    tx: &mut Transaction<'_, Postgres>,
    book_id: i64,
    actor: i64,
) -> Result<(), AuthorError> {
    sqlx::query(
        "WITH first_releases AS (
           INSERT INTO chapter_first_release_events
             (chapter_id,version_id,creator_user_id)
           SELECT c.id,p.version_id,$2 FROM chapters c
           JOIN chapter_publications p ON p.chapter_id=c.id
           WHERE c.book_id=$1 AND c.visibility='public'
             AND c.access_level='free' AND c.price_amount=0
             AND NOT c.admin_hold AND NOT c.is_deleted
             AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
           ON CONFLICT(chapter_id) DO NOTHING
           RETURNING chapter_id,version_id
         )
         INSERT INTO outbox_events
           (event_type,schema_version,aggregate_kind,aggregate_id,
            dedupe_key,payload,required_consumers)
         SELECT 'ChapterPublished',1,'chapter',r.chapter_id,
                'chapter-published:' || r.chapter_id::text || ':' || r.version_id::text,
                jsonb_build_object(
                    'chapter_id',r.chapter_id,'book_id',$1::bigint,
                    'version_id',r.version_id,
                    'published_at',clock_timestamp(),'first_release',true),
                ARRAY['chapter_release_fanout_v1']::text[]
         FROM first_releases r
         WHERE true
         ON CONFLICT(dedupe_key) DO NOTHING",
    )
    .bind(book_id)
    .bind(actor)
    .execute(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    Ok(())
}
