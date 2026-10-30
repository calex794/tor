//! Publish durable ChapterPublished envelopes in B's owning transaction.
//! The outbox consumer effects belong to G/F/E; B only records the fact.
use crate::services::authoring::AuthorError;
use sqlx::{Postgres, Transaction};

/// One immutable chapter/version pair has one deduplication key. Replaying
/// the same command cannot send another event; no chapter body is stored.
pub(super) async fn enqueue_chapter_published(
    tx: &mut Transaction<'_, Postgres>,
    book_id: i64,
    chapter_id: i64,
    version_id: i64,
    first_release: bool,
) -> Result<(), AuthorError> {
    sqlx::query(
        "INSERT INTO outbox_events
           (event_type,schema_version,aggregate_kind,aggregate_id,
            dedupe_key,payload,required_consumers)
         VALUES ('ChapterPublished',1,'chapter',$1,
                 'chapter-published:' || $1::text || ':' || $2::text,
                 jsonb_build_object(
                    'chapter_id',$1,'book_id',$3,'version_id',$2,
                    'published_at',now(),'first_release',$4),
                 ARRAY['search-index-v1','subscription-inbox-v1']::text[])
         ON CONFLICT (dedupe_key) DO NOTHING",
    )
    .bind(chapter_id)
    .bind(version_id)
    .bind(book_id)
    .bind(first_release)
    .execute(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    Ok(())
}
