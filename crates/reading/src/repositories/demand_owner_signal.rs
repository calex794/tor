//! RET-R03: one private owner-bell event for a genuine demand frontier.
//! This is written in the same transaction as the new verified request.
//! No requester identity or book text enters the shared outbox.
use super::DemandError;
use sqlx::{Postgres, Transaction};

pub(super) async fn enqueue(
    tx: &mut Transaction<'_, Postgres>,
    book_id: i64,
    frontier_chapter_id: i64,
    requester_id: i64,
) -> Result<(), DemandError> {
    sqlx::query(
        r#"INSERT INTO outbox_events
             (event_type,schema_version,aggregate_kind,aggregate_id,
              dedupe_key,payload,required_consumers)
           SELECT 'reader.continuation.requested',1,'book',book.id,
                  format('ret03:continuation:%s:%s:%s',
                         book.id,$2::bigint,book.owner_user_id),
                  jsonb_build_object(
                    'actor_user_id',0,
                    'recipient_user_id',book.owner_user_id,
                    'object_id',book.id,
                    'frontier_chapter_id',$2::bigint
                  ),
                  ARRAY['user_notifications']::text[]
           FROM books AS book
           JOIN users AS owner ON owner.id=book.owner_user_id
           WHERE book.id=$1 AND book.owner_user_id<>$3
             AND owner.account_state='active'
             AND book.visibility='public' AND book.visibility_status='active'
             AND book.lifecycle_status<>'completed'
             AND NOT book.admin_hold AND NOT book.is_deleted
           ON CONFLICT(dedupe_key) DO NOTHING"#,
    )
    .bind(book_id)
    .bind(frontier_chapter_id)
    .bind(requester_id)
    .execute(&mut **tx)
    .await
    .map_err(|_| DemandError::Unavailable)?;
    Ok(())
}
