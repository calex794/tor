//! C-owned first started event, called only after a validated monotone CAS.
//! The caller has already locked B/D, checked the exact readable version and
//! validated the position against text or the immutable ReaderVersionManifest.
use crate::services::sync::SyncError;
use sqlx::{Postgres, Transaction};

pub(super) async fn record(
    tx: &mut Transaction<'_, Postgres>,
    actor: i64,
    book: i64,
    chapter: i64,
    version: i64,
    purchase: Option<i64>,
) -> Result<(), SyncError> {
    sqlx::query(
        "WITH started AS (
           INSERT INTO reader_chapter_starts(user_id,book_id,chapter_id,first_version_id,purchase_id,source_event_id)
           SELECT $1,$2,$3,$4,$5,gen_random_uuid()
           WHERE (($5::bigint IS NULL AND EXISTS(
             SELECT 1 FROM reader_book_positions p WHERE p.user_id=$1 AND p.book_id=$2
               AND p.chapter_id=$3 AND p.version_id=$4 AND (p.block_index>1 OR p.char_offset>0)))
             OR ($5::bigint IS NOT NULL AND EXISTS(
             SELECT 1 FROM reader_purchased_positions p WHERE p.user_id=$1 AND p.purchase_id=$5
               AND p.chapter_id=$3 AND p.version_id=$4 AND (p.block_index>1 OR p.char_offset>0))))
           ON CONFLICT(user_id,book_id,chapter_id) DO NOTHING RETURNING source_event_id
         ) INSERT INTO outbox_events(id,event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,payload,required_consumers)
         SELECT source_event_id,'ChapterStartedV1',1,'chapter',$3,
           concat('chapter-started:',$1,':',$3),
           jsonb_strip_nulls(jsonb_build_object('user_id',$1,'book_id',$2,'chapter_id',$3,
             'version_id',$4,'purchase_id',$5,
             'authority',CASE WHEN $5::bigint IS NULL THEN 'C-verified-free-start' ELSE 'C-verified-purchased-start' END)),
           ARRAY['recommendation-feedback-v1'] FROM started",
    )
    .bind(actor)
    .bind(book)
    .bind(chapter)
    .bind(version)
    .bind(purchase)
    .execute(&mut **tx)
    .await
    .map_err(|_| SyncError::Unavailable)?;
    Ok(())
}
