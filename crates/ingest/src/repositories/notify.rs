//! G-08 SQLx adapter: bounded recipient enumeration and fenced on-site staging.
use crate::services::notify::{RecipientError, RecipientPage, RECIPIENT_BATCH_SIZE};
use sqlx::{PgPool, Row};

/// Shared SQL for read-only and durable paged scans.
const RECIPIENTS_SQL: &str = r#"SELECT follow.user_id
             FROM outbox_events AS event
             JOIN chapters AS chapter ON chapter.id=event.aggregate_id
               AND chapter.id=(event.payload->>'chapter_id')::bigint
             JOIN books AS book ON book.id=chapter.book_id
               AND book.id=(event.payload->>'book_id')::bigint
             JOIN chapter_publications AS publication
               ON publication.chapter_id=chapter.id
              AND publication.version_id=(event.payload->>'version_id')::bigint
             JOIN reader_book_follows AS follow ON follow.book_id=book.id
             WHERE event.id=$1::uuid
               AND event.event_type='ChapterPublished'
               AND event.schema_version=1
               AND event.aggregate_kind='chapter'
               AND event.dead_at IS NULL AND event.delivered_at IS NULL
               AND follow.user_id>$2
               AND follow.created_at<=(event.payload->>'published_at')::timestamptz
               AND book.visibility='public'
               AND NOT book.admin_hold AND NOT book.is_deleted
               AND chapter.visibility='public'
               AND NOT chapter.admin_hold AND NOT chapter.is_deleted
               AND (chapter.reader_released_at IS NULL OR chapter.reader_released_at<=now())
               AND (
                   (chapter.access_level='free' AND chapter.price_amount=0)
                   OR (
                       chapter.access_level='paid' AND chapter.price_amount>0
                       AND EXISTS (
                           SELECT 1
                           FROM chapter_encrypted_release_events AS encrypted
                           JOIN chapter_versions AS version
                             ON version.id=encrypted.version_id
                            AND version.chapter_id=encrypted.chapter_id
                           JOIN storage_objects AS object
                             ON object.id=encrypted.storage_object_id
                            AND object.id=version.storage_object_id
                           WHERE encrypted.chapter_id=chapter.id
                             AND encrypted.version_id=publication.version_id
                             AND object.state='attached'
                             AND object.object_kind='chapter'
                       )
                   )
               )
             ORDER BY follow.user_id ASC
             LIMIT 101"#;

#[derive(Clone)]
pub struct PgNotificationRecipients {
    pool: PgPool,
}

impl PgNotificationRecipients {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Returns at most 100 subscriber IDs with an exclusive keyset cursor.
    /// Time eligibility is based on the specific published VERSION, not the
    /// first-release-only C-16 site feed. A post-release follow is excluded.
    ///
    /// Querying candidates does not record a delivery, mark an Inbox entry
    /// read, create entitlements, or override an opt-out / external consent.
    pub async fn next_page(
        &self,
        event_id: &str,
        after_user_id: Option<i64>,
    ) -> Result<RecipientPage, RecipientError> {
        if event_id.len() != 36 || after_user_id.is_some_and(|id| id <= 0) {
            return Err(RecipientError::Invalid);
        }
        // Do not silently turn an unsupported event into a successful empty
        // delivery. The producer owns the versioned and typed payload.
        let valid: Option<bool> = sqlx::query_scalar(
            "SELECT COALESCE(
                    event_type='ChapterPublished'
                    AND schema_version=1 AND aggregate_kind='chapter'
                    AND 'subscription-inbox-v1'=ANY(required_consumers)
                    AND jsonb_typeof(payload->'chapter_id')='number'
                    AND (payload->>'chapter_id')=aggregate_id::text
                    AND jsonb_typeof(payload->'book_id')='number'
                    AND jsonb_typeof(payload->'version_id')='number'
                    AND jsonb_typeof(payload->'published_at')='string'
                    AND jsonb_typeof(payload->'first_release')='boolean',
                    false)
             FROM outbox_events
             WHERE id=$1::uuid AND dead_at IS NULL AND delivered_at IS NULL",
        )
        .bind(event_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| RecipientError::Unavailable)?;
        if valid != Some(true) {
            return Err(RecipientError::Invalid);
        }

        // Existing (book_id,user_id) follow index and keyset prevent offset
        // scans. One extra row tells us whether to request the next 100.
        // Recompute current visibility on each batch; later unfollows are
        // excluded and re-follows cannot receive historical versions.
        let mut user_ids: Vec<i64> = sqlx::query_scalar(RECIPIENTS_SQL)
            .bind(event_id)
            .bind(after_user_id.unwrap_or(0))
            .fetch_all(&self.pool)
            .await
            .map_err(|_| RecipientError::Unavailable)?;
        let has_more = user_ids.len() > RECIPIENT_BATCH_SIZE;
        user_ids.truncate(RECIPIENT_BATCH_SIZE);
        let next_user_id = if has_more {
            user_ids.last().copied()
        } else {
            None
        };
        Ok(RecipientPage {
            user_ids,
            next_user_id,
        })
    }

    /// Stage at most 100 internal on-site ID-intents and advance the durable
    /// cursor in the same transaction, fenced by the current outbox lease.
    /// This does NOT represent email/push delivery or channel consent.
    pub async fn stage_next(
        &self,
        event_id: &str,
        lease_epoch: i64,
    ) -> Result<crate::services::notify::StagedPage, RecipientError> {
        use crate::services::notify::StagedPage;

        if event_id.len() != 36 || lease_epoch <= 0 {
            return Err(RecipientError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| RecipientError::Unavailable)?;
        let authorized: Option<bool> = sqlx::query_scalar(
            "SELECT COALESCE(
                event_type='ChapterPublished' AND schema_version=1
                AND aggregate_kind='chapter'
                AND 'subscription-inbox-v1'=ANY(required_consumers)
                AND jsonb_typeof(payload->'chapter_id')='number'
                AND (payload->>'chapter_id')=aggregate_id::text
                AND jsonb_typeof(payload->'book_id')='number'
                AND jsonb_typeof(payload->'version_id')='number'
                AND jsonb_typeof(payload->'published_at')='string'
                AND jsonb_typeof(payload->'first_release')='boolean',
                false)
             FROM outbox_events
             WHERE id=$1::uuid AND lease_epoch=$2 AND lease_until>now()
               AND dead_at IS NULL AND delivered_at IS NULL
             FOR UPDATE",
        )
        .bind(event_id)
        .bind(lease_epoch)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| RecipientError::Unavailable)?;
        match authorized {
            Some(true) => (),
            Some(false) => return Err(RecipientError::Invalid),
            None => return Err(RecipientError::LostLease),
        }
        sqlx::query(
            "INSERT INTO notification_event_fanout_scans(event_id)
             VALUES($1::uuid) ON CONFLICT(event_id) DO NOTHING",
        )
        .bind(event_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| RecipientError::Unavailable)?;
        let checkpoint = sqlx::query(
            "SELECT cursor_user_id,completed_at IS NOT NULL AS complete
             FROM notification_event_fanout_scans
             WHERE event_id=$1::uuid FOR UPDATE",
        )
        .bind(event_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| RecipientError::Unavailable)?;
        let previous: i64 = checkpoint
            .try_get("cursor_user_id")
            .map_err(|_| RecipientError::Unavailable)?;
        let done: bool = checkpoint
            .try_get("complete")
            .map_err(|_| RecipientError::Unavailable)?;
        if done {
            return Ok(StagedPage {
                newly_staged: 0,
                cursor_user_id: previous,
                complete: true,
            });
        }

        let mut user_ids: Vec<i64> = sqlx::query_scalar(RECIPIENTS_SQL)
            .bind(event_id)
            .bind(previous)
            .fetch_all(&mut *tx)
            .await
            .map_err(|_| RecipientError::Unavailable)?;
        let complete = user_ids.len() <= RECIPIENT_BATCH_SIZE;
        user_ids.truncate(RECIPIENT_BATCH_SIZE);
        let cursor = user_ids.last().copied().unwrap_or(previous);
        let staged = sqlx::query(
            "INSERT INTO notification_event_recipient_intents(event_id,user_id)
             SELECT $1::uuid, recipients.user_id
             FROM unnest($2::bigint[]) AS recipients(user_id)
             WHERE true
             ON CONFLICT(event_id,user_id) DO NOTHING",
        )
        .bind(event_id)
        .bind(&user_ids)
        .execute(&mut *tx)
        .await
        .map_err(|_| RecipientError::Unavailable)?
        .rows_affected();
        sqlx::query(
            "UPDATE notification_event_fanout_scans
             SET cursor_user_id=$2,updated_at=now(),
                 completed_at=CASE WHEN $3 THEN now() ELSE NULL END
             WHERE event_id=$1::uuid",
        )
        .bind(event_id)
        .bind(cursor)
        .bind(complete)
        .execute(&mut *tx)
        .await
        .map_err(|_| RecipientError::Unavailable)?;
        if complete {
            // Only a durable on-site metadata projection is acknowledged.
            // An external transport must have its own verified consumer.
            sqlx::query(
                "INSERT INTO event_deliveries(consumer,event_id)
                 VALUES('subscription-inbox-v1',$1::uuid)
                 ON CONFLICT(consumer,event_id) DO NOTHING",
            )
            .bind(event_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| RecipientError::Unavailable)?;
            sqlx::query(
                "UPDATE outbox_events AS event
                 SET delivered_at=now(),lease_until=NULL
                 WHERE event.id=$1::uuid AND event.lease_epoch=$2
                   AND event.lease_until>now()
                   AND event.delivered_at IS NULL AND event.dead_at IS NULL
                   AND NOT EXISTS (
                     SELECT 1 FROM unnest(event.required_consumers) AS req(consumer)
                     WHERE NOT EXISTS (
                       SELECT 1 FROM event_deliveries AS receipt
                       WHERE receipt.event_id=event.id
                         AND receipt.consumer=req.consumer
                     )
                   )",
            )
            .bind(event_id)
            .bind(lease_epoch)
            .execute(&mut *tx)
            .await
            .map_err(|_| RecipientError::Unavailable)?;
            // If search has not acknowledged yet, release the finished G08
            // lease so the independent consumer need not wait 60 seconds.
            sqlx::query(
                "UPDATE outbox_events SET lease_until=NULL
                 WHERE id=$1::uuid AND lease_epoch=$2 AND lease_until>now()
                   AND delivered_at IS NULL AND dead_at IS NULL",
            )
            .bind(event_id)
            .bind(lease_epoch)
            .execute(&mut *tx)
            .await
            .map_err(|_| RecipientError::Unavailable)?;
        } else {
            // Renew the same fenced lease without incrementing attempts:
            // one large event may require many hundred-recipient batches.
            sqlx::query(
                "UPDATE outbox_events SET lease_until=now()+interval '60 seconds'
                 WHERE id=$1::uuid AND lease_epoch=$2 AND lease_until>now()",
            )
            .bind(event_id)
            .bind(lease_epoch)
            .execute(&mut *tx)
            .await
            .map_err(|_| RecipientError::Unavailable)?;
        }
        tx.commit().await.map_err(|_| RecipientError::Unavailable)?;
        Ok(StagedPage {
            newly_staged: staged,
            cursor_user_id: cursor,
            complete,
        })
    }
}
