//! S058: bounded, durable ChapterPublished fanout into E-08's existing
//! per-recipient outbox pipeline. No plaintext and no external transport.
use super::mail::unavailable;
use crate::domain::MailError;
use sqlx::PgPool;

const RECIPIENTS_PER_BATCH: usize = 250;
const CONSUMER: &str = "chapter_release_fanout_v1";

/// Generate per-reader child events, not notifications, under the source
/// event lock. A crash rolls back the batch and its cursor together.
pub(super) async fn fanout(pool: &PgPool, limit: u16) -> Result<usize, MailError> {
    if !(1..=100).contains(&limit) {
        return Err(MailError::Invalid);
    }
    let mut processed = 0;
    for _ in 0..limit {
        let mut tx = pool.begin().await.map_err(unavailable)?;
        let source: Option<(String, i64, i64, i64)> = sqlx::query_as(
            "SELECT e.id::text,e.aggregate_id,
                    (e.payload->>'book_id')::bigint,
                    (e.payload->>'version_id')::bigint
             FROM outbox_events e
             WHERE e.event_type='ChapterPublished'
               AND e.delivered_at IS NULL AND e.dead_at IS NULL
               AND e.required_consumers @> ARRAY['chapter_release_fanout_v1']::text[]
               AND e.next_attempt_at<=clock_timestamp()
               AND NOT EXISTS (
                   SELECT 1 FROM event_deliveries d
                   WHERE d.event_id=e.id
                     AND d.consumer='chapter_release_fanout_v1')
             ORDER BY e.created_at,e.id
             FOR UPDATE SKIP LOCKED LIMIT 1",
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        let Some((event_id, chapter_id, book_id, version_id)) = source else {
            tx.commit().await.map_err(unavailable)?;
            break;
        };

        // An ordinary release may be held or withdrawn before fanout. Do not
        // notify readers of a hidden chapter; C-16 still independently
        // projects currently visible followed releases at read time.
        let owner: Option<i64> = sqlx::query_scalar(
            "SELECT b.owner_user_id FROM books b
             JOIN chapters c ON c.book_id=b.id
             JOIN chapter_publications p ON p.chapter_id=c.id
             JOIN chapter_first_release_events first
               ON first.chapter_id=c.id AND first.version_id=$3
             WHERE b.id=$1 AND c.id=$2
               AND b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
               AND c.visibility='public' AND NOT c.admin_hold AND NOT c.is_deleted
               AND (c.reader_released_at IS NULL
                    OR c.reader_released_at<=clock_timestamp())
               AND (
                    (c.access_level='free' AND c.price_amount=0)
                    OR (c.access_level='paid' AND c.price_amount>0
                        AND EXISTS (
                          SELECT 1 FROM chapter_encrypted_release_events e
                          JOIN storage_objects o ON o.id=e.storage_object_id
                          WHERE e.chapter_id=c.id AND e.version_id=p.version_id
                            AND o.state='attached' AND o.object_kind='chapter'
                            AND o.verified_at IS NOT NULL))
               )",
        )
        .bind(book_id)
        .bind(chapter_id)
        .bind(version_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;

        let mut more = false;
        if let Some(owner_id) = owner {
            let cursor: i64 = sqlx::query_scalar(
                "SELECT last_user_id FROM chapter_release_notification_fanout
                 WHERE source_event_id=$1::uuid",
            )
            .bind(&event_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(unavailable)?
            .unwrap_or(0);
            // NEwBOO targets active library readers; TOR additionally has
            // explicit book follows. Union both intents, dedupe dual members
            // and never backfill follows or library saves made after release.
            let readers: Vec<i64> = sqlx::query_scalar(
                "SELECT intents.user_id FROM (
                    SELECT user_id,created_at FROM reader_book_follows
                    WHERE book_id=$2
                    UNION ALL
                    SELECT user_id,created_at FROM reader_library_items
                    WHERE book_id=$2
                 ) intents
                 JOIN users u ON u.id=intents.user_id AND u.account_state='active'
                 JOIN outbox_events root ON root.id=$1::uuid
                 WHERE intents.user_id>$3 AND intents.user_id<>$4
                   AND intents.created_at<=root.created_at
                 GROUP BY intents.user_id
                 ORDER BY intents.user_id
                 LIMIT 251",
            )
            .bind(&event_id)
            .bind(book_id)
            .bind(cursor)
            .bind(owner_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(unavailable)?;
            more = readers.len() > RECIPIENTS_PER_BATCH;
            let recipients: Vec<i64> = readers.into_iter().take(RECIPIENTS_PER_BATCH).collect();
            if !recipients.is_empty() {
                sqlx::query(
                    "INSERT INTO outbox_events (
                       event_type,schema_version,aggregate_kind,aggregate_id,
                       dedupe_key,payload,required_consumers
                     )
                     SELECT 'reader.chapter.released',1,'chapter',$1,
                       'chapter-released-recipient:' || $1::text || ':' || r.user_id::text,
                       jsonb_build_object(
                           'actor_user_id',$2,
                           'recipient_user_id',r.user_id,
                           'object_id',$1,
                           'book_id',$3,
                           'version_id',$4
                       ),
                       ARRAY['user_notifications']::text[]
                     FROM unnest($5::bigint[]) AS r(user_id)
                     ON CONFLICT(dedupe_key) DO NOTHING",
                )
                .bind(chapter_id)
                .bind(owner_id)
                .bind(book_id)
                .bind(version_id)
                .bind(&recipients)
                .execute(&mut *tx)
                .await
                .map_err(unavailable)?;
                let last = *recipients.last().ok_or(MailError::Unavailable)?;
                sqlx::query(
                    "INSERT INTO chapter_release_notification_fanout(
                        source_event_id,last_user_id)
                     VALUES($1::uuid,$2)
                     ON CONFLICT(source_event_id) DO UPDATE
                     SET last_user_id=EXCLUDED.last_user_id,
                         updated_at=clock_timestamp()",
                )
                .bind(&event_id)
                .bind(last)
                .execute(&mut *tx)
                .await
                .map_err(unavailable)?;
            }
        }
        if !more {
            // One receipt for the root, irrespective of recipient count.
            // Child events have individual user_notifications receipts.
            sqlx::query(
                "INSERT INTO event_deliveries(consumer,event_id)
                 VALUES($1,$2::uuid) ON CONFLICT DO NOTHING",
            )
            .bind(CONSUMER)
            .bind(&event_id)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
            sqlx::query(
                "UPDATE outbox_events e SET delivered_at=clock_timestamp()
                 WHERE e.id=$1::uuid AND NOT EXISTS(
                     SELECT 1 FROM unnest(e.required_consumers) AS required(consumer)
                     WHERE NOT EXISTS (
                         SELECT 1 FROM event_deliveries d
                         WHERE d.event_id=e.id AND d.consumer=required.consumer))",
            )
            .bind(&event_id)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        }
        tx.commit().await.map_err(unavailable)?;
        processed += 1;
    }
    Ok(processed)
}
