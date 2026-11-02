use super::identity;
use crate::{
    domain::{Chat, Message, MessagePage, Requester, SupportError},
    services::SupportRepository,
};
use async_trait::async_trait;
use sqlx::{PgPool, Postgres, Transaction};

#[derive(Clone)]
pub struct PgSupport {
    pub(super) pool: PgPool,
}
pub(super) fn unavailable(_: sqlx::Error) -> SupportError {
    SupportError::Unavailable
}
impl PgSupport {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    pub async fn cleanup_expired(&self, limit: u16) -> Result<u64, SupportError> {
        if !(1..=200).contains(&limit) {
            return Err(SupportError::Invalid);
        }
        sqlx::query(
            "DELETE FROM support_conversations WHERE id IN(SELECT id FROM support_conversations
           WHERE expires_at<=now() ORDER BY expires_at,id FOR UPDATE SKIP LOCKED LIMIT $1)",
        )
        .bind(i64::from(limit))
        .execute(&self.pool)
        .await
        .map(|r| r.rows_affected())
        .map_err(unavailable)
    }
}

pub(super) async fn page(
    tx: &mut Transaction<'_, Postgres>,
    chat: i64,
    _admin: Option<i64>,
    after: Option<i64>,
    before: Option<i64>,
    limit: u16,
) -> Result<MessagePage, SupportError> {
    let (requester_id,guest_alias):(Option<i64>,String)=sqlx::query_as("SELECT requester_user_id,guest_alias FROM support_conversations WHERE id=$1 AND (expires_at IS NULL OR expires_at>now()) FOR UPDATE")
        .bind(chat).fetch_optional(&mut **tx).await.map_err(unavailable)?.ok_or(SupportError::NotFound)?;
    let mut rows=sqlx::query_as::<_,(i64,String,String,String)>(r#"SELECT id,sender_kind,body,to_char(created_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
       FROM support_messages WHERE conversation_id=$1 AND ($2::BIGINT IS NULL OR id>$2) AND ($3::BIGINT IS NULL OR id<$3)
       ORDER BY CASE WHEN $2::BIGINT IS NOT NULL THEN id END ASC, CASE WHEN $2::BIGINT IS NULL THEN id END DESC LIMIT $4"#)
        .bind(chat).bind(after).bind(before).bind(i64::from(limit)+1).fetch_all(&mut **tx).await.map_err(unavailable)?;
    let has_more = rows.len() > usize::from(limit);
    rows.truncate(usize::from(limit));
    if after.is_none() {
        rows.reverse();
    }
    Ok(MessagePage {
        chat: Chat {
            id: chat,
            requester_id,
            guest_alias,
            last_message_id: rows.last().map(|m| m.0),
            last_message_preview: String::new(),
            unread: false,
        },
        messages: rows
            .into_iter()
            .map(|r| Message {
                id: r.0,
                sender_kind: r.1,
                body: r.2,
                created_at: r.3,
            })
            .collect(),
        has_more,
    })
}

pub(super) async fn send(
    tx: &mut Transaction<'_, Postgres>,
    chat: i64,
    kind: &str,
    user: Option<i64>,
    body: &str,
    key: Option<&str>,
) -> Result<Message, SupportError> {
    let scope: String = if let Some(user) = user {
        format!("{kind}:{user}")
    } else {
        sqlx::query_scalar("SELECT 'guest:' || encode(guest_identity_hash,'hex') FROM support_conversations WHERE id=$1")
            .bind(chat).fetch_one(&mut **tx).await.map_err(unavailable)?
    };
    if let Some(key) = key {
        let existing=sqlx::query_as::<_,(i64,String,String,String)>(r#"SELECT id,sender_kind,body,to_char(created_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') FROM support_messages WHERE conversation_id=$1 AND sender_scope=$2 AND client_message_id=$3"#)
            .bind(chat).bind(&scope).bind(key).fetch_optional(&mut **tx).await.map_err(unavailable)?;
        if let Some(r) = existing {
            return Ok(Message {
                id: r.0,
                sender_kind: r.1,
                body: r.2,
                created_at: r.3,
            });
        }
    }
    let r=sqlx::query_as::<_,(i64,String,String,String)>(r#"INSERT INTO support_messages(conversation_id,sender_kind,sender_scope,sender_user_id,body,client_message_id)
       VALUES($1,$2,$3,$4,$5,$6) RETURNING id,sender_kind,body,to_char(created_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"')"#)
        .bind(chat).bind(kind).bind(scope).bind(user).bind(body).bind(key).fetch_one(&mut **tx).await.map_err(unavailable)?;
    sqlx::query("UPDATE support_conversations SET updated_at=now(),expires_at=CASE WHEN requester_user_id IS NULL THEN now()+INTERVAL '30 days' ELSE NULL END WHERE id=$1")
        .bind(chat).execute(&mut **tx).await.map_err(unavailable)?;
    Ok(Message {
        id: r.0,
        sender_kind: r.1,
        body: r.2,
        created_at: r.3,
    })
}

#[async_trait]
impl SupportRepository for PgSupport {
    async fn requester_messages(
        &self,
        who: &Requester,
        after: Option<i64>,
        before: Option<i64>,
        limit: u16,
    ) -> Result<MessagePage, SupportError> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let chat = identity::ensure(&mut tx, who).await?;
        let result = page(&mut tx, chat, None, after, before, limit).await?;
        tx.commit().await.map_err(unavailable)?;
        Ok(result)
    }
    async fn send_requester(
        &self,
        who: &Requester,
        body: &str,
        key: Option<&str>,
    ) -> Result<Message, SupportError> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let chat = identity::ensure(&mut tx, who).await?;
        // Claim/creation holds the chat lock before allocating message IDs.
        let (kind, user) = match who {
            Requester::Guest(_) => ("guest", None),
            Requester::User { id, .. } => ("user", Some(*id)),
        };
        let result = send(&mut tx, chat, kind, user, body, key).await?;
        tx.commit().await.map_err(unavailable)?;
        Ok(result)
    }
    async fn admin_chats(
        &self,
        admin: i64,
        before: Option<i64>,
        limit: u16,
    ) -> Result<Vec<Chat>, SupportError> {
        let rows=sqlx::query_as::<_,(i64,Option<i64>,String,Option<i64>,String,bool)>("WITH chats AS (
          SELECT c.id,c.requester_user_id,c.guest_alias,m.id AS last_id,COALESCE(left(m.body,140),'') AS preview,
            (m.id IS NOT NULL AND (r.last_message_id IS NULL OR m.id>r.last_message_id)) AS unread,
            COALESCE(m.created_at,c.created_at) AS activity
          FROM support_conversations c LEFT JOIN LATERAL(SELECT id,body,created_at FROM support_messages WHERE conversation_id=c.id ORDER BY id DESC LIMIT 1)m ON TRUE
          LEFT JOIN support_admin_reads r ON r.conversation_id=c.id AND r.admin_user_id=$1
          WHERE c.expires_at IS NULL OR c.expires_at>now())
          SELECT id,requester_user_id,guest_alias,last_id,preview,unread FROM chats
          WHERE $2::BIGINT IS NULL OR (activity,id)<(SELECT activity,id FROM chats WHERE id=$2)
          ORDER BY activity DESC,id DESC LIMIT $3")
            .bind(admin).bind(before).bind(i64::from(limit)).fetch_all(&self.pool).await.map_err(unavailable)?;
        Ok(rows
            .into_iter()
            .map(|r| Chat {
                id: r.0,
                requester_id: r.1,
                guest_alias: r.2,
                last_message_id: r.3,
                last_message_preview: r.4,
                unread: r.5,
            })
            .collect())
    }
    async fn admin_messages(
        &self,
        admin: i64,
        chat: i64,
        after: Option<i64>,
        before: Option<i64>,
        limit: u16,
    ) -> Result<MessagePage, SupportError> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let result = page(&mut tx, chat, Some(admin), after, before, limit).await?;
        tx.commit().await.map_err(unavailable)?;
        Ok(result)
    }
    async fn send_admin(
        &self,
        admin: i64,
        chat: i64,
        body: &str,
        key: Option<&str>,
        request_id: &str,
    ) -> Result<Message, SupportError> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        // The same row lock serializes admin/user sends and guest claims.
        page(&mut tx, chat, None, None, None, 1).await?;
        let result = send(&mut tx, chat, "admin", Some(admin), body, key).await?;
        sqlx::query("INSERT INTO admin_action_audit(actor_id,action,resource_type,resource_id,request_id,outcome)
          VALUES($1,'support.reply','support_message',$2,$3,'success')
          ON CONFLICT(resource_id) WHERE action='support.reply' AND outcome='success' DO NOTHING")
            .bind(admin).bind(result.id).bind(request_id).execute(&mut *tx).await.map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        Ok(result)
    }
    async fn mark_admin_read(
        &self,
        admin: i64,
        chat: i64,
        message: i64,
        request_id: &str,
    ) -> Result<(), SupportError> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        page(&mut tx, chat, None, None, None, 1).await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM support_messages WHERE conversation_id=$1 AND id=$2)",
        )
        .bind(chat)
        .bind(message)
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        if !exists {
            return Err(SupportError::NotFound);
        }
        let changed=sqlx::query("INSERT INTO support_admin_reads(conversation_id,admin_user_id,last_message_id) VALUES($1,$2,$3)
          ON CONFLICT(conversation_id,admin_user_id) DO UPDATE SET last_message_id=EXCLUDED.last_message_id
          WHERE support_admin_reads.last_message_id<EXCLUDED.last_message_id")
            .bind(chat).bind(admin).bind(message).execute(&mut *tx).await.map_err(unavailable)?.rows_affected();
        if changed > 0 {
            sqlx::query("INSERT INTO admin_action_audit(actor_id,action,resource_type,resource_id,request_id,outcome) VALUES($1,'support.read','support_conversation',$2,$3,'success')")
                .bind(admin).bind(chat).bind(request_id).execute(&mut *tx).await.map_err(unavailable)?;
        }
        tx.commit().await.map_err(unavailable)
    }
}
