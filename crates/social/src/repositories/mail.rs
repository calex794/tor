use crate::{
    domain::{Conversation, MailError, MessagePage, SentMessage},
    services::MailRepository,
};
use async_trait::async_trait;
use sqlx::{PgPool, Postgres, Transaction};

#[derive(Clone)]
pub struct PgMail {
    pub(super) pool: PgPool,
}

pub(super) fn unavailable(_: sqlx::Error) -> MailError {
    MailError::Unavailable
}

pub(super) fn foreign_or_unavailable(error: sqlx::Error) -> MailError {
    if error.as_database_error().and_then(|e| e.code()).as_deref() == Some("23503") {
        MailError::NotFound
    } else {
        MailError::Unavailable
    }
}

pub(super) async fn pair_lock(
    tx: &mut Transaction<'_, Postgres>,
    a: i64,
    b: i64,
) -> Result<(), MailError> {
    // Same key and lock order for start, send and block, across all processes.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('tor-social-pair:' || $1::BIGINT::TEXT || ':' || $2::BIGINT::TEXT,0))")
        .bind(a.min(b)).bind(a.max(b)).execute(&mut **tx).await.map_err(unavailable)?;
    Ok(())
}

pub(super) async fn blocked(
    tx: &mut Transaction<'_, Postgres>,
    a: i64,
    b: i64,
) -> Result<bool, MailError> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM social_user_blocks WHERE (blocker_id=$1 AND blocked_id=$2) OR (blocker_id=$2 AND blocked_id=$1))")
        .bind(a).bind(b).fetch_one(&mut **tx).await.map_err(unavailable)
}

impl PgMail {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub(super) async fn start_command(&self, actor: i64, peer: i64) -> Result<i64, MailError> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('tor-social-sender:' || $1::BIGINT::TEXT,0))")
            .bind(actor).execute(&mut *tx).await.map_err(unavailable)?;
        pair_lock(&mut tx, actor, peer).await?;
        if blocked(&mut tx, actor, peer).await? {
            return Err(MailError::Blocked);
        }
        if let Some(id) = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM social_conversations WHERE user_low=$1 AND user_high=$2",
        )
        .bind(actor.min(peer))
        .bind(actor.max(peer))
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?
        {
            tx.commit().await.map_err(unavailable)?;
            return Ok(id);
        }
        let recent:i64=sqlx::query_scalar("SELECT count(*) FROM social_conversations WHERE created_by=$1 AND created_at>clock_timestamp()-INTERVAL '1 minute'")
            .bind(actor).fetch_one(&mut *tx).await.map_err(unavailable)?;
        if recent >= 10 {
            return Err(MailError::RateLimited);
        }
        // Foreign keys are the account-existence authority. No private account
        // rows, emails or profile state are read by this feature.
        let id = sqlx::query_scalar("INSERT INTO social_conversations(user_low,user_high,created_by) VALUES($1,$2,$3) ON CONFLICT(user_low,user_high) DO UPDATE SET user_low=EXCLUDED.user_low RETURNING id")
            .bind(actor.min(peer)).bind(actor.max(peer)).bind(actor).fetch_one(&mut *tx).await.map_err(foreign_or_unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        Ok(id)
    }

    pub(super) async fn mailbox_query(
        &self,
        actor: i64,
        after: Option<i64>,
        limit: u16,
    ) -> Result<Vec<Conversation>, MailError> {
        let rows = sqlx::query_as::<_,(i64,i64,i64,bool)>(
            "SELECT c.id, CASE WHEN c.user_low=$1 THEN c.user_high ELSE c.user_low END,
             (SELECT count(*) FROM social_messages m WHERE m.conversation_id=c.id AND m.sender_id<>$1 AND m.id>COALESCE(r.through_id,0)),
             EXISTS(SELECT 1 FROM social_user_blocks b WHERE (b.blocker_id=c.user_low AND b.blocked_id=c.user_high) OR (b.blocker_id=c.user_high AND b.blocked_id=c.user_low))
             FROM social_conversations c LEFT JOIN social_mail_reads r ON r.conversation_id=c.id AND r.user_id=$1
             WHERE $1 IN(c.user_low,c.user_high) AND ($2::BIGINT IS NULL OR c.id<$2)
             ORDER BY c.id DESC LIMIT $3")
            .bind(actor).bind(after).bind(i64::from(limit)).fetch_all(&self.pool).await.map_err(unavailable)?;
        Ok(rows
            .into_iter()
            .map(|(id, peer_id, unread, blocked)| Conversation {
                id,
                peer_id,
                unread,
                blocked,
            })
            .collect())
    }

    pub(super) async fn read_command(
        &self,
        actor: i64,
        conversation: i64,
        through: i64,
    ) -> Result<(), MailError> {
        let changed = sqlx::query_scalar::<_,i64>(
            "INSERT INTO social_mail_reads(conversation_id,user_id,through_id)
             SELECT c.id,$1,m.id FROM social_conversations c JOIN social_messages m ON m.conversation_id=c.id
             WHERE c.id=$2 AND m.id=$3 AND $1 IN(c.user_low,c.user_high)
             ON CONFLICT(conversation_id,user_id) DO UPDATE SET through_id=GREATEST(social_mail_reads.through_id,EXCLUDED.through_id)
             RETURNING through_id")
            .bind(actor).bind(conversation).bind(through).fetch_optional(&self.pool).await.map_err(unavailable)?;
        changed.map(|_| ()).ok_or(MailError::NotFound)
    }

    pub(super) async fn block_command(
        &self,
        actor: i64,
        peer: i64,
        value: bool,
    ) -> Result<(), MailError> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        pair_lock(&mut tx, actor, peer).await?;
        if value {
            sqlx::query("INSERT INTO social_user_blocks(blocker_id,blocked_id) VALUES($1,$2) ON CONFLICT DO NOTHING")
                .bind(actor).bind(peer).execute(&mut *tx).await.map_err(foreign_or_unavailable)?;
        } else {
            // Never remove the other user's independent block.
            sqlx::query("DELETE FROM social_user_blocks WHERE blocker_id=$1 AND blocked_id=$2")
                .bind(actor)
                .bind(peer)
                .execute(&mut *tx)
                .await
                .map_err(unavailable)?;
        }
        tx.commit().await.map_err(unavailable)
    }
}

#[async_trait]
impl MailRepository for PgMail {
    async fn start(&self, actor: i64, peer: i64) -> Result<i64, MailError> {
        self.start_command(actor, peer).await
    }
    async fn mailbox(
        &self,
        actor: i64,
        after: Option<i64>,
        limit: u16,
    ) -> Result<Vec<Conversation>, MailError> {
        self.mailbox_query(actor, after, limit).await
    }
    async fn messages(
        &self,
        actor: i64,
        conversation: i64,
        after: Option<i64>,
        before: Option<i64>,
        limit: u16,
    ) -> Result<MessagePage, MailError> {
        self.messages_query(actor, conversation, after, before, limit)
            .await
    }
    async fn send(
        &self,
        actor: i64,
        conversation: i64,
        body: &str,
        key: &str,
    ) -> Result<SentMessage, MailError> {
        self.send_command(actor, conversation, body, key).await
    }
    async fn mark_read(
        &self,
        actor: i64,
        conversation: i64,
        through: i64,
    ) -> Result<(), MailError> {
        self.read_command(actor, conversation, through).await
    }
    async fn set_block(&self, actor: i64, peer: i64, value: bool) -> Result<(), MailError> {
        self.block_command(actor, peer, value).await
    }
}
