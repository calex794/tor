//! E-08/S057 read-only administrator mail storage. Not wired into HTTP.
//! Every operation binds a live session digest, locks its administrator role
//! against revocation, and commits an immutable audit record before returning.
//! Never accept a claimed administrator user ID or return authentication data.
use crate::domain::MailError;
use sqlx::{PgPool, Postgres, Transaction};

#[derive(Clone)]
pub struct PgAdminMail {
    pool: PgPool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailUserRow {
    pub user_id: i64,
    pub display_name: String,
    pub conversation_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminDirectConversation {
    pub id: i64,
    pub user_low: i64,
    pub user_high: i64,
    pub message_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminDirectMessage {
    pub id: i64,
    pub sender_id: i64,
    pub sender_name: String,
    pub body: String,
}

fn unavailable(_: sqlx::Error) -> MailError {
    MailError::Unavailable
}

async fn authorize(
    tx: &mut Transaction<'_, Postgres>,
    digest: &[u8; 32],
) -> Result<i64, MailError> {
    // The same transaction holds the session, active account and admin role.
    // A concurrent role revocation or session termination must wait for
    // this read/audit to finish; the next request sees revoked permissions.
    let admin: Option<i64> = sqlx::query_scalar(
        "SELECT u.id FROM sessions s
         JOIN users u ON u.id=s.user_id
         JOIN account_roles r ON r.user_id=u.id
         WHERE s.token_hash=$1 AND s.revoked_at IS NULL
           AND s.expires_at>now() AND u.account_state='active'
           AND r.role='administrator' AND r.is_active
         FOR SHARE OF s,u,r",
    )
    .bind(digest.as_slice())
    .fetch_optional(&mut **tx)
    .await
    .map_err(unavailable)?;
    admin.ok_or(MailError::NotFound)
}

async fn record(
    tx: &mut Transaction<'_, Postgres>,
    admin: i64,
    action: &str,
    target_user: Option<i64>,
    conversation: Option<i64>,
    count: usize,
) -> Result<(), MailError> {
    let count = i32::try_from(count).map_err(|_| MailError::Unavailable)?;
    sqlx::query(
        "INSERT INTO social_mail_admin_access_audit
         (admin_user_id,action,target_user_id,conversation_id,result_count)
         VALUES($1,$2,$3,$4,$5)",
    )
    .bind(admin)
    .bind(action)
    .bind(target_user)
    .bind(conversation)
    .bind(count)
    .execute(&mut **tx)
    .await
    .map_err(unavailable)?;
    Ok(())
}

impl PgAdminMail {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Bounded display-name search; no email or credential material returned.
    /// The actor is derived solely from the session digest under a role lock.
    pub async fn users(
        &self,
        digest: &[u8; 32],
        query: &str,
        before: Option<i64>,
        limit: u16,
    ) -> Result<Vec<MailUserRow>, MailError> {
        let query = query.trim();
        if query.chars().count() > 80
            || before.is_some_and(|id| id <= 0)
            || !(1..=50).contains(&limit)
        {
            return Err(MailError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let admin = authorize(&mut tx, digest).await?;
        let rows = sqlx::query_as::<_, (i64, String, i64)>(
            "SELECT u.id,u.display_name,
                (SELECT count(*) FROM social_conversations c
                 WHERE c.user_low=u.id OR c.user_high=u.id)
             FROM users u
             WHERE ($1::bigint IS NULL OR u.id<$1)
               AND strpos(lower(u.display_name),lower($2::text))>0
             ORDER BY u.id DESC LIMIT $3",
        )
        .bind(before)
        .bind(query)
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await
        .map_err(unavailable)?;
        record(&mut tx, admin, "search_mail_users", None, None, rows.len()).await?;
        tx.commit().await.map_err(unavailable)?;
        Ok(rows
            .into_iter()
            .map(|(user_id, display_name, conversation_count)| MailUserRow {
                user_id,
                display_name,
                conversation_count,
            })
            .collect())
    }

    /// List *direct* mail belonging to the selected user. Group and public
    /// room identities are separate and intentionally out of scope here.
    pub async fn mailbox(
        &self,
        digest: &[u8; 32],
        user_id: i64,
        before: Option<i64>,
        limit: u16,
    ) -> Result<Vec<AdminDirectConversation>, MailError> {
        if user_id <= 0
            || before.is_some_and(|id| id <= 0)
            || !(1..=50).contains(&limit)
        {
            return Err(MailError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let admin = authorize(&mut tx, digest).await?;
        let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE id=$1")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(unavailable)?;
        if exists.is_none() {
            return Err(MailError::NotFound);
        }
        let rows = sqlx::query_as::<_, (i64, i64, i64, i64)>(
            "SELECT c.id,c.user_low,c.user_high,
                 (SELECT count(*) FROM social_messages m WHERE m.conversation_id=c.id)
             FROM social_conversations c
             WHERE $1 IN (c.user_low,c.user_high)
               AND ($2::bigint IS NULL OR c.id<$2)
             ORDER BY c.id DESC LIMIT $3",
        )
        .bind(user_id)
        .bind(before)
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await
        .map_err(unavailable)?;
        record(
            &mut tx,
            admin,
            "list_user_mailbox",
            Some(user_id),
            None,
            rows.len(),
        )
        .await?;
        tx.commit().await.map_err(unavailable)?;
        Ok(rows
            .into_iter()
            .map(
                |(id, user_low, user_high, message_count)| AdminDirectConversation {
                    id,
                    user_low,
                    user_high,
                    message_count,
                },
            )
            .collect())
    }

    /// Message bodies are returned only after live session+role validation.
    /// The audit INSERT is in the same transaction; failed audit means no data.
    pub async fn messages(
        &self,
        digest: &[u8; 32],
        conversation_id: i64,
        before: Option<i64>,
        limit: u16,
    ) -> Result<Vec<AdminDirectMessage>, MailError> {
        if conversation_id <= 0
            || before.is_some_and(|id| id <= 0)
            || !(1..=50).contains(&limit)
        {
            return Err(MailError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let admin = authorize(&mut tx, digest).await?;
        let exists: Option<i64> =
            sqlx::query_scalar("SELECT id FROM social_conversations WHERE id=$1")
                .bind(conversation_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(unavailable)?;
        if exists.is_none() {
            return Err(MailError::NotFound);
        }
        let mut rows = sqlx::query_as::<_, (i64, i64, String, String)>(
            "SELECT m.id,m.sender_id,u.display_name,m.body
             FROM social_messages m JOIN users u ON u.id=m.sender_id
             WHERE m.conversation_id=$1
               AND ($2::bigint IS NULL OR m.id<$2)
             ORDER BY m.id DESC LIMIT $3",
        )
        .bind(conversation_id)
        .bind(before)
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await
        .map_err(unavailable)?;
        record(
            &mut tx,
            admin,
            "read_direct_conversation",
            None,
            Some(conversation_id),
            rows.len(),
        )
        .await?;
        tx.commit().await.map_err(unavailable)?;
        rows.reverse();
        Ok(rows
            .into_iter()
            .map(
                |(id, sender_id, sender_name, body)| AdminDirectMessage {
                    id,
                    sender_id,
                    sender_name,
                    body,
                },
            )
            .collect())
    }
}
