//! #661: explicit, internal-only delivery of still-actionable overdue
//! observations to the existing on-site bell. Not a scheduled job and never
//! an external email/push notification. Business recipients are not approved.
use super::mail::unavailable;
use crate::domain::MailError;
use sqlx::PgPool;

/// No automatic caller is registered. The operator must approve recipient
/// policy and delivery cadence before a worker is allowed to invoke this.
pub(super) async fn project(pool: &PgPool, limit: u16) -> Result<usize, MailError> {
    if !(1..=100).contains(&limit) {
        return Err(MailError::Invalid);
    }
    let mut tx = pool.begin().await.map_err(unavailable)?;
    // Independent of the observation writer's advisory lock; projection
    // cannot double-send when processes race or restart.
    sqlx::query("SELECT pg_advisory_xact_lock(661, 2028)")
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
    type Projection = (String, i64, String, i16);
    let inserted: Vec<Projection> = sqlx::query_as(
        "WITH eligible AS (
           SELECT s.id,s.request_kind,s.source_id,s.source_revision,
                  s.threshold_hours,admin.user_id
           FROM moderation_overdue_signals s
           JOIN account_roles admin
             ON admin.role='administrator' AND admin.is_active
           JOIN users recipient ON recipient.id=admin.user_id
             AND recipient.account_state='active'
           WHERE s.signal_state='open'
             AND NOT EXISTS (
               SELECT 1 FROM user_notification_preferences p
               WHERE p.user_id=admin.user_id AND p.kind='moderation_overdue'
                 AND NOT p.enabled
             )
             AND (
               (s.request_kind='registration' AND s.source_revision=0
                AND EXISTS (
                  SELECT 1 FROM books b WHERE b.id=s.source_id
                    AND b.registration_requires_review
                    AND b.visibility_status='pending_moderation'
                    AND b.registration_approval_decision_id IS NULL
                    AND NOT b.is_deleted
                    AND b.created_at<=now()-make_interval(hours=>s.threshold_hours::integer)
                    AND NOT EXISTS (
                      SELECT 1 FROM book_registration_decisions d WHERE d.book_id=b.id
                    )
                ))
               OR (s.request_kind='cover' AND s.source_revision=0
                AND EXISTS (
                  SELECT 1 FROM book_cover_review_requests c
                  JOIN books b ON b.id=c.book_id
                  WHERE c.id=s.source_id AND c.state='pending' AND NOT b.is_deleted
                    AND c.submitted_at<=now()-make_interval(hours=>s.threshold_hours::integer)
                ))
               OR (s.request_kind='deletion'
                AND EXISTS (
                  SELECT 1 FROM book_ordinary_deletion_requests r
                  JOIN books b ON b.id=r.book_id
                  WHERE r.book_id=s.source_id AND r.request_revision=s.source_revision
                    AND r.owner_user_id<>admin.user_id
                    AND b.lifecycle_status='pending_deletion'
                    AND b.lifecycle_revision=r.request_revision
                    AND NOT b.is_deleted
                    AND r.requested_at<=now()-make_interval(hours=>s.threshold_hours::integer)
                    AND NOT EXISTS (
                      SELECT 1 FROM book_ordinary_deletion_decisions d
                      WHERE d.book_id=r.book_id AND d.request_revision=r.request_revision
                    )
                ))
             )
             AND NOT EXISTS (
               SELECT 1 FROM outbox_events e
               WHERE e.dedupe_key='mod-overdue-bell:' || s.id || ':' || admin.user_id
             )
           ORDER BY s.id,admin.user_id
           LIMIT $1
         ),
         inserted AS (
           INSERT INTO outbox_events
             (event_type,schema_version,aggregate_kind,aggregate_id,
              dedupe_key,payload,required_consumers)
           SELECT 'moderation.overdue',1,'moderation_overdue_signal',e.id,
             'mod-overdue-bell:' || e.id || ':' || e.user_id,
             jsonb_build_object(
               'recipient_user_id',e.user_id,
               'source_id',e.source_id,
               'source_revision',e.source_revision,
               'request_kind',e.request_kind,
               'threshold_hours',e.threshold_hours
             ),
             ARRAY['moderation_overdue_bell_v1']::text[]
           FROM eligible e
           ON CONFLICT(dedupe_key) DO NOTHING
           RETURNING id,aggregate_id,payload
         )
         SELECT id::text,(payload->>'recipient_user_id')::bigint,
                payload->>'request_kind',(payload->>'threshold_hours')::smallint
         FROM inserted",
    )
    .bind(i64::from(limit))
    .fetch_all(&mut *tx)
    .await
    .map_err(unavailable)?;

    for (event_id, recipient, kind, hours) in &inserted {
        let label = match kind.as_str() {
            "registration" => "Регистрация книги",
            "cover" => "Обложка",
            "deletion" => "Удаление книги",
            _ => return Err(MailError::Unavailable),
        };
        // The outbox source, in-app projection, and receipt commit together.
        // No replay can create another user-facing record.
        sqlx::query(
            "INSERT INTO user_notifications
                 (recipient_user_id,actor_user_id,source_event_id,
                  kind,title,body,url)
             VALUES($1,NULL,$2::uuid,'moderation_overdue',$3,$4,
                    '/moderation/books/alerts')
             ON CONFLICT(source_event_id) DO NOTHING",
        )
        .bind(recipient)
        .bind(event_id)
        .bind("Ожидает модерации")
        .bind(format!("{label}: заявка ожидает от {hours} часов"))
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        sqlx::query(
            "INSERT INTO event_deliveries(consumer,event_id)
             VALUES('moderation_overdue_bell_v1',$1::uuid)
             ON CONFLICT DO NOTHING",
        )
        .bind(event_id)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        sqlx::query(
            "UPDATE outbox_events SET delivered_at=clock_timestamp()
             WHERE id=$1::uuid AND NOT EXISTS (
               SELECT 1 FROM unnest(required_consumers) AS required(consumer)
               WHERE NOT EXISTS (
                 SELECT 1 FROM event_deliveries d
                 WHERE d.event_id=$1::uuid AND d.consumer=required.consumer
               )
             )",
        )
        .bind(event_id)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
    }
    tx.commit().await.map_err(unavailable)?;
    Ok(inserted.len())
}
