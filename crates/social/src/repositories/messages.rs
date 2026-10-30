use super::{mail::{blocked, pair_lock, unavailable, PgMail}, notification_events};
use crate::domain::{MailError, Message, MessagePage, SentMessage};

impl PgMail {
    pub(super) async fn messages_query(
        &self,
        actor: i64,
        conversation: i64,
        after: Option<i64>,
        before: Option<i64>,
        limit: u16,
    ) -> Result<MessagePage, MailError> {
        let (peer, is_blocked, by_me) = sqlx::query_as::<_,(i64,bool,bool)>(
            "SELECT CASE WHEN c.user_low=$1 THEN c.user_high ELSE c.user_low END,
             EXISTS(SELECT 1 FROM social_user_blocks b WHERE (b.blocker_id=c.user_low AND b.blocked_id=c.user_high) OR (b.blocker_id=c.user_high AND b.blocked_id=c.user_low)),
             EXISTS(SELECT 1 FROM social_user_blocks b WHERE b.blocker_id=$1 AND b.blocked_id IN(c.user_low,c.user_high))
             FROM social_conversations c WHERE c.id=$2 AND $1 IN(c.user_low,c.user_high)")
            .bind(actor).bind(conversation).fetch_optional(&self.pool).await.map_err(unavailable)?.ok_or(MailError::NotFound)?;
        let rows = sqlx::query_as::<_,(i64,i64,String,String,String)>(
            r#"SELECT m.id,m.sender_id,m.body,to_char(m.created_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"'),
                      to_char((m.created_at AT TIME ZONE 'UTC')+INTERVAL '3 hours','DD.MM.YYYY HH24:MI')
               FROM social_messages m JOIN social_conversations c ON c.id=m.conversation_id
               WHERE c.id=$2 AND $1 IN(c.user_low,c.user_high)
                 AND ($3::BIGINT IS NULL OR m.id>$3) AND ($4::BIGINT IS NULL OR m.id<$4)
               ORDER BY CASE WHEN $3::BIGINT IS NOT NULL THEN m.id END ASC,
                        CASE WHEN $3::BIGINT IS NULL THEN m.id END DESC LIMIT $5"#)
            .bind(actor).bind(conversation).bind(after).bind(before).bind(i64::from(limit)+1)
            .fetch_all(&self.pool).await.map_err(unavailable)?;
        let has_more = rows.len() > usize::from(limit);
        let mut items: Vec<_> = rows
            .into_iter()
            .take(usize::from(limit))
            .map(
                |(id, sender_id, body, created_at, created_at_display)| Message {
                    id,
                    sender_id,
                    body,
                    created_at,
                    created_at_display,
                },
            )
            .collect();
        if after.is_none() {
            items.reverse();
        }
        Ok(MessagePage {
            items,
            peer_id: peer,
            blocked: is_blocked,
            blocked_by_me: by_me,
            has_more,
        })
    }

    pub(super) async fn send_command(
        &self,
        actor: i64,
        conversation: i64,
        body: &str,
        key: &str,
        dedup_recent: bool,
        client_id: Option<&str>,
    ) -> Result<SentMessage, MailError> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        // Per-sender serialization protects cross-conversation TOR command identity.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('tor-social-sender:' || $1::BIGINT::TEXT,0))")
            .bind(actor).execute(&mut *tx).await.map_err(unavailable)?;
        let (low,high) = sqlx::query_as::<_,(i64,i64)>("SELECT user_low,user_high FROM social_conversations WHERE id=$1 AND $2 IN(user_low,user_high)")
            .bind(conversation).bind(actor).fetch_optional(&mut *tx).await.map_err(unavailable)?.ok_or(MailError::NotFound)?;
        pair_lock(&mut tx, low, high).await?;
        if blocked(&mut tx, low, high).await? {
            return Err(MailError::Blocked);
        }
        // Source HTML identity is per sender and conversation, not TOR JSON global key.
        // The pair lock serializes concurrent retries and blocking.
        if let Some(client_id) = client_id {
            if let Some(id) = sqlx::query_scalar::<_, i64>(
                "SELECT id FROM social_messages WHERE conversation_id=$1 AND sender_id=$2 AND client_message_id=$3"
            ).bind(conversation).bind(actor).bind(client_id)
                .fetch_optional(&mut *tx).await.map_err(unavailable)? {
                tx.commit().await.map_err(unavailable)?;
                return Ok(SentMessage { id, replayed: true });
            }
        }
        // NEwBOO mail_service.py: absent client ID scans the last 20 messages,
        // returning the latest same-sender/body message aged 0..5 seconds.
        // The pair lock also makes this fallback safe across server processes.
        if dedup_recent {
            if let Some(id) = sqlx::query_scalar::<_, i64>(
                "SELECT id FROM (SELECT id,sender_id,body,created_at FROM social_messages
                 WHERE conversation_id=$1 ORDER BY id DESC LIMIT 20) recent
                 WHERE sender_id=$2 AND body=$3
                   AND created_at BETWEEN statement_timestamp()-INTERVAL '5 seconds' AND statement_timestamp()
                 ORDER BY id DESC LIMIT 1")
                .bind(conversation).bind(actor).bind(body)
                .fetch_optional(&mut *tx).await.map_err(unavailable)? {
                tx.commit().await.map_err(unavailable)?;
                return Ok(SentMessage { id, replayed: true });
            }
        }
        // Return the committed receipt before inserting another message.
        if let Some((id,original_conversation,original_body)) = sqlx::query_as::<_,(i64,i64,String)>("SELECT id,conversation_id,body FROM social_messages WHERE sender_id=$1 AND command_key=$2")
            .bind(actor).bind(key).fetch_optional(&mut *tx).await.map_err(unavailable)? {
            if original_conversation!=conversation || original_body!=body { return Err(MailError::Conflict); }
            tx.commit().await.map_err(unavailable)?;
            return Ok(SentMessage{id,replayed:true});
        }
        // IDs are allocated under the conversation's pair lock, so a committed
        // polling cursor cannot skip a later commit with an earlier message ID.
        let id = sqlx::query_scalar("INSERT INTO social_messages(conversation_id,sender_id,body,command_key,client_message_id) VALUES($1,$2,$3,$4,$5) RETURNING id")
            .bind(conversation).bind(actor).bind(body).bind(key).bind(client_id).fetch_one(&mut *tx).await.map_err(unavailable)?;
        let recipient = if actor == low { high } else { low };
        let dedupe = format!("tor-social:mail-message:{id}");
        notification_events::queue(
            &mut tx, "social.mail.created", "social_message",
            id, actor, recipient, conversation, Some(&dedupe),
        ).await?;
        tx.commit().await.map_err(unavailable)?;
        Ok(SentMessage {
            id,
            replayed: false,
        })
    }
}
