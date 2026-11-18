//! RET-R03: inspect a completed request's site-owned outbox signal.
#![forbid(unsafe_code)]
use super::super::mail::unavailable;
use crate::domain::MailError;
use sqlx::{Postgres, Transaction};

pub(super) struct Snapshot {
    pub title: String,
    pub waiting: i64,
}

/// The recipient must still own this book and the original frontier must
/// still be its last published chapter. No reader ID is read or returned.
pub(super) async fn current(
    tx: &mut Transaction<'_, Postgres>,
    event_id: &str,
    owner_id: i64,
    book_id: i64,
    frontier_id: Option<i64>,
) -> Result<Option<Snapshot>, MailError> {
    let Some(frontier_id) = frontier_id.filter(|id| *id > 0) else {
        return Ok(None);
    };
    let row: Option<(String, i64)> = sqlx::query_as(
        r#"SELECT b.title,
                  (SELECT count(*) FROM reader_continuation_demand demand
                   WHERE demand.book_id=b.id
                     AND demand.frontier_chapter_id=$3)::bigint
           FROM outbox_events event
           JOIN books b ON b.id=event.aggregate_id
           JOIN users owner ON owner.id=b.owner_user_id
           JOIN LATERAL (
               SELECT chapter.id
               FROM chapters chapter
               JOIN chapter_publications publication ON publication.chapter_id=chapter.id
               WHERE chapter.book_id=b.id
                 AND chapter.visibility='public'
                 AND NOT chapter.admin_hold AND NOT chapter.is_deleted
                 AND (chapter.reader_released_at IS NULL
                      OR chapter.reader_released_at<=now())
               ORDER BY chapter.chapter_no DESC,chapter.id DESC LIMIT 1
           ) frontier ON frontier.id=$3
           WHERE event.id=$4::uuid
             AND event.event_type='reader.continuation.requested'
             AND event.aggregate_kind='book'
             AND event.aggregate_id=$1
             AND event.payload->>'actor_user_id'='0'
             AND event.payload->>'recipient_user_id'=$2::text
             AND event.payload->>'object_id'=$1::text
             AND event.payload->>'frontier_chapter_id'=$3::text
             AND b.id=$1 AND b.owner_user_id=$2
             AND b.visibility='public' AND b.visibility_status='active'
             AND b.lifecycle_status<>'completed'
             AND NOT b.admin_hold AND NOT b.is_deleted
             AND owner.account_state='active'
             AND EXISTS (
                 SELECT 1 FROM reader_continuation_demand demand
                 WHERE demand.book_id=b.id AND demand.frontier_chapter_id=$3
             )"#,
    )
    .bind(book_id)
    .bind(owner_id)
    .bind(frontier_id)
    .bind(event_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(unavailable)?;
    Ok(row.map(|(title, waiting)| Snapshot { title, waiting }))
}
