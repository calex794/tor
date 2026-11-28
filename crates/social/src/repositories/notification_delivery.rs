//! E-08/RET-R03: durable in-app projection of shared outbox events.
//! Keeping the consumer separate preserves the notification reader's size cap.
use super::super::mail::{blocked, pair_lock, unavailable};
use super::{actor_text, continuation};
use crate::domain::MailError;
use sqlx::PgPool;

pub(super) async fn pending(pool: &PgPool, limit: u16) -> Result<usize, MailError> {
    if !(1..=100).contains(&limit) {
        return Err(MailError::Invalid);
    }
    let mut processed = 0;
    for _ in 0..limit {
        let mut tx = pool.begin().await.map_err(unavailable)?;
        let event = sqlx::query_as::<_, (String, String, i64, i64, i64, Option<i64>)>(
            "SELECT e.id::text,e.event_type,
                    (e.payload->>'actor_user_id')::bigint,
                    (e.payload->>'recipient_user_id')::bigint,
                    (e.payload->>'object_id')::bigint,
                    (e.payload->>'frontier_chapter_id')::bigint
             FROM outbox_events e
             WHERE e.delivered_at IS NULL AND e.dead_at IS NULL
               AND e.next_attempt_at<=clock_timestamp()
               AND (e.lease_until IS NULL OR e.lease_until<=clock_timestamp())
               AND e.required_consumers @> ARRAY['user_notifications']::text[]
               AND e.event_type IN
                 ('social.mail.created','social.friendship.requested',
                  'social.friendship.accepted','social.friendship.removed',
                  'social.group_mail.created',
                  'social.group_creator_transfer.proposed',
                  'reader.chapter.released','reader.chapter.updated',
                  'reader.continuation.requested')
               AND NOT EXISTS (
                 SELECT 1 FROM event_deliveries d
                 WHERE d.event_id=e.id AND d.consumer='user_notifications')
             ORDER BY e.created_at,e.id
             FOR UPDATE SKIP LOCKED LIMIT 1",
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        let Some((event_id, kind, actor, recipient, object_id, frontier)) = event else {
            tx.commit().await.map_err(unavailable)?;
            break;
        };
        // One anonymous, deduplicated book-frontier signal is allowed only
        // while its intended recipient still owns the live book.
        let owner_demand = if kind == "reader.continuation.requested" {
            continuation::current(&mut tx, &event_id, recipient, object_id, frontier).await?
        } else {
            None
        };
        // A block committed before consumption suppresses the notification,
        // never the message. Valid actor/recipient membership is authoritative.
        let user_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM users WHERE id=$1) AND
                    EXISTS(SELECT 1 FROM users WHERE id=$2)",
        )
        .bind(actor)
        .bind(recipient)
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        // Use the same pair lock as send/block; a committed block cannot
        // race past an already-checked notification projection.
        if user_exists && actor != recipient && actor > 0 && recipient > 0 {
            pair_lock(&mut tx, actor, recipient).await?;
        }
        let is_blocked = if user_exists && actor != recipient && actor > 0 && recipient > 0 {
            blocked(&mut tx, actor, recipient).await?
        } else {
            true
        };
        let member = if kind == "reader.continuation.requested" {
            owner_demand.is_some()
        } else if kind == "social.mail.created" {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM social_conversations
                 WHERE id=$1 AND $2 IN(user_low,user_high)
                   AND $3 IN(user_low,user_high) AND $2<>$3)",
            )
            .bind(object_id)
            .bind(actor)
            .bind(recipient)
            .fetch_one(&mut *tx)
            .await
            .map_err(unavailable)?
        } else if kind == "social.group_mail.created" {
            // A departed member must not receive a delayed notification.
            // FOR SHARE serializes projection against member exit, and the
            // source message check binds this event to the actual sender.
            let status: Option<String> = sqlx::query_scalar(
                "SELECT m.status FROM social_group_members m
                 WHERE m.group_id=$1 AND m.user_id=$2
                   AND EXISTS(
                     SELECT 1 FROM social_group_messages msg
                     JOIN outbox_events e ON e.id=$4::uuid
                     WHERE msg.group_id=$1 AND msg.id=e.aggregate_id
                       AND msg.sender_id=$3)
                 FOR SHARE",
            )
            .bind(object_id)
            .bind(recipient)
            .bind(actor)
            .bind(&event_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(unavailable)?;
            status.as_deref() == Some("active")
        } else if kind == "reader.chapter.released" || kind == "reader.chapter.updated" {
            super::super::chapter_notifications::eligible_recipient(
                &mut tx, object_id, actor, recipient, &event_id,
            )
            .await?
        } else if kind == "social.group_creator_transfer.proposed" {
            // Lock the group in a separate statement BEFORE checking
            // the offer. With READ COMMITTED, joining a locked group in
            // the offer query could use a stale, unlocked offer snapshot
            // after a concurrent revoke/decline/accept commits.
            // Every transfer mutation takes the group lock first.
            let group_locked = sqlx::query_scalar::<_, i64>(
                "SELECT id FROM social_group_conversations WHERE id=$1 FOR SHARE",
            )
            .bind(object_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(unavailable)?
            .is_some();
            let pending: Option<i64> = if group_locked {
                sqlx::query_scalar(
                    "SELECT t.id FROM social_group_creator_transfers t
                     JOIN social_group_conversations g ON g.id=t.group_id
                     JOIN social_group_members owner ON owner.group_id=t.group_id
                       AND owner.user_id=t.from_user_id AND owner.status='active'
                       AND owner.role='creator'
                     JOIN social_group_members member ON member.group_id=t.group_id
                       AND member.user_id=t.to_user_id AND member.status='active'
                     JOIN users sender ON sender.id=t.from_user_id
                       AND sender.account_state='active'
                     JOIN users receiver ON receiver.id=t.to_user_id
                       AND receiver.account_state='active'
                     JOIN outbox_events e ON e.id=$4::uuid AND e.aggregate_id=t.id
                     WHERE t.group_id=$1 AND t.from_user_id=$3 AND t.to_user_id=$2
                       AND t.state='pending' AND t.expires_at>clock_timestamp()
                     FOR SHARE OF t,owner,member,sender,receiver",
                )
                .bind(object_id)
                .bind(recipient)
                .bind(actor)
                .bind(&event_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(unavailable)?
            } else {
                None
            };
            pending.is_some()
        } else {
            object_id == actor
        };
        let allowed = if kind == "reader.continuation.requested" {
            owner_demand.is_some() && recipient > 0 && member
        } else {
            user_exists && !is_blocked && actor > 0 && recipient > 0 && actor != recipient && member
        };
        if allowed {
            let (category, title, body, url) = if kind == "reader.continuation.requested" {
                let signal = owner_demand.as_ref().expect("validated owner demand");
                let short_title: String = signal.title.chars().take(110).collect();
                (
                    "continuation_demand",
                    "Читатели ждут продолжения",
                    format!("Книга «{short_title}». Запросов на продолжение: {}. Подробнее — в статистике.", signal.waiting),
                    "/me/stats/continuations".to_owned(),
                )
            } else {
                let (name, handle): (Option<String>, Option<String>) = sqlx::query_as(
                    "SELECT CASE WHEN u.account_state='active'
                                  AND strpos(lower(u.display_name),u.email_normalized)=0
                                 THEN u.display_name END,
                            CASE WHEN u.account_state='active' THEN s.handle END
                     FROM users u LEFT JOIN account_settings s ON s.user_id=u.id
                     WHERE u.id=$1",
                )
                .bind(actor)
                .fetch_one(&mut *tx)
                .await
                .map_err(unavailable)?;
                actor_text::project(&kind, name.as_deref(), handle.as_deref(), Some(object_id))
                    .expect("event_type is closed by SQL")
            };
            sqlx::query(
                "INSERT INTO user_notifications
                   (recipient_user_id,actor_user_id,source_event_id,
                    kind,title,body,url,created_at)
                 SELECT $1,$2,e.id,$3,$4,$5,$6,e.created_at
                 FROM outbox_events e WHERE e.id=$7::uuid
                   AND NOT EXISTS (
                     SELECT 1 FROM user_notification_preferences p
                     WHERE p.user_id=$1 AND p.kind=$3 AND NOT p.enabled)
                 ON CONFLICT(source_event_id) DO NOTHING",
            )
            .bind(recipient)
            .bind((kind != "reader.continuation.requested").then_some(actor))
            .bind(category)
            .bind(title)
            .bind(body)
            .bind(url)
            .bind(&event_id)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        }
        sqlx::query(
            "INSERT INTO event_deliveries(consumer,event_id)
             VALUES('user_notifications',$1::uuid) ON CONFLICT DO NOTHING",
        )
        .bind(&event_id)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        sqlx::query(
            "UPDATE outbox_events o SET delivered_at=clock_timestamp()
             WHERE o.id=$1::uuid AND NOT EXISTS(
                 SELECT 1 FROM unnest(o.required_consumers) AS required(consumer)
                 WHERE NOT EXISTS(
                     SELECT 1 FROM event_deliveries d
                     WHERE d.event_id=o.id AND d.consumer=required.consumer))",
        )
        .bind(&event_id)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        processed += 1;
    }
    Ok(processed)
}
