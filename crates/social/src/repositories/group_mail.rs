//! E-08/S057 group-mail storage, isolated from immutable direct-mail pairs.
//! No public route yet: group membership and actor access must be proven first.
use crate::domain::{clean_body, valid_key, MailError, SentMessage};
use sqlx::PgPool;

#[derive(Clone)]
pub struct PgGroupMail {
    pool: PgPool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GroupConversation {
    pub id: i64,
    pub title: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GroupMessage {
    pub id: i64,
    pub sender_id: i64,
    pub body: String,
}

fn unavailable(_: sqlx::Error) -> MailError {
    MailError::Unavailable
}

fn missing_or_unavailable(error: sqlx::Error) -> MailError {
    match error.as_database_error().and_then(|e| e.code()).as_deref() {
        Some("23503") => MailError::NotFound,
        _ => MailError::Unavailable,
    }
}

impl PgGroupMail {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Source start_group_conversation: creator is an included member,
    /// duplicates do not create extra seats, and each group is independent.
    /// Operational cap 32 is stricter than NEwBOO and requires parity review.
    pub async fn create(
        &self,
        actor: i64,
        member_ids: &[i64],
        title: &str,
    ) -> Result<i64, MailError> {
        let title = title.trim();
        let mut members = member_ids.to_vec();
        members.push(actor);
        members.sort_unstable();
        members.dedup();
        if actor <= 0
            || !(2..=32).contains(&members.len())
            || members.iter().any(|id| *id <= 0)
            || title.is_empty()
            || title.chars().count() > 120
            || title.chars().any(char::is_control)
        {
            return Err(MailError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let found: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM users
             WHERE id=ANY($1::bigint[]) AND account_state='active'",
        )
        .bind(&members)
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        if found != members.len() as i64 {
            return Err(MailError::NotFound);
        }
        // Group invitation must not bypass either participant's direct block.
        // Reuse the pair locks from the direct-mail block command and take
        // them in a deterministic order before checking both directions.
        for &member in &members {
            if member != actor {
                super::mail::pair_lock(&mut tx, actor, member).await?;
            }
        }
        let is_blocked: bool = sqlx::query_scalar(
            "SELECT EXISTS(
               SELECT 1 FROM social_user_blocks b
               WHERE (b.blocker_id=$1 AND b.blocked_id=ANY($2::bigint[]))
                  OR (b.blocked_id=$1 AND b.blocker_id=ANY($2::bigint[]))
             )",
        )
        .bind(actor)
        .bind(&members)
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        if is_blocked {
            return Err(MailError::Blocked);
        }
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO social_group_conversations(created_by,title)
             VALUES($1,$2) RETURNING id",
        )
        .bind(actor)
        .bind(title)
        .fetch_one(&mut *tx)
        .await
        .map_err(missing_or_unavailable)?;
        sqlx::query(
            "INSERT INTO social_group_members(group_id,user_id,role,status)
             SELECT $1,member_id,
                    CASE WHEN member_id=$2 THEN 'creator' ELSE 'member' END,
                    'active'
             FROM unnest($3::bigint[]) AS participants(member_id)",
        )
        .bind(id)
        .bind(actor)
        .bind(&members)
        .execute(&mut *tx)
        .await
        .map_err(missing_or_unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        Ok(id)
    }

    /// Only active members can see even the group title or existence.
    /// Cursor filtering happens after the actor filter, never before it.
    pub async fn list(
        &self,
        actor: i64,
        before: Option<i64>,
        limit: u16,
    ) -> Result<Vec<GroupConversation>, MailError> {
        if actor <= 0 || before.is_some_and(|n| n <= 0) || !(1..=50).contains(&limit) {
            return Err(MailError::Invalid);
        }
        let rows = sqlx::query_as::<_, (i64, String)>(
            "SELECT g.id,g.title FROM social_group_conversations g
             JOIN social_group_members m ON m.group_id=g.id
             WHERE m.user_id=$1 AND m.status='active'
               AND ($2::bigint IS NULL OR g.id<$2)
             ORDER BY g.id DESC LIMIT $3",
        )
        .bind(actor)
        .bind(before)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(unavailable)?;
        Ok(rows
            .into_iter()
            .map(|(id, title)| GroupConversation { id, title })
            .collect())
    }

    /// The member lock serializes page reads with membership exit/revocation.
    /// Nonmembers and departed members get the same NotFound answer.
    pub async fn messages(
        &self,
        actor: i64,
        group_id: i64,
        before: Option<i64>,
        limit: u16,
    ) -> Result<Vec<GroupMessage>, MailError> {
        if actor <= 0
            || group_id <= 0
            || before.is_some_and(|n| n <= 0)
            || !(1..=50).contains(&limit)
        {
            return Err(MailError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let status: Option<String> = sqlx::query_scalar(
            "SELECT status FROM social_group_members
             WHERE group_id=$1 AND user_id=$2 FOR SHARE",
        )
        .bind(group_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if status.as_deref() != Some("active") {
            return Err(MailError::NotFound);
        }
        let mut rows = sqlx::query_as::<_, (i64, i64, String)>(
            "SELECT id,sender_id,body FROM social_group_messages
             WHERE group_id=$1 AND ($2::bigint IS NULL OR id<$2)
             ORDER BY id DESC LIMIT $3",
        )
        .bind(group_id)
        .bind(before)
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        rows.reverse();
        Ok(rows
            .into_iter()
            .map(|(id, sender_id, body)| GroupMessage {
                id,
                sender_id,
                body,
            })
            .collect())
    }

    /// A send and a leave lock the same membership row. The command-key lock
    /// serializes retries from one sender across processes and groups.
    pub async fn send(
        &self,
        actor: i64,
        group_id: i64,
        body: &str,
        key: &str,
    ) -> Result<SentMessage, MailError> {
        if actor <= 0 || group_id <= 0 || !valid_key(key) {
            return Err(MailError::Invalid);
        }
        let body = clean_body(body)?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let status: Option<String> = sqlx::query_scalar(
            "SELECT status FROM social_group_members
             WHERE group_id=$1 AND user_id=$2 FOR UPDATE",
        )
        .bind(group_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if status.as_deref() != Some("active") {
            return Err(MailError::NotFound);
        }
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtextextended(
                'tor-social-group-key:' || $1::bigint::text || ':' || $2::text,0))",
        )
        .bind(actor)
        .bind(key)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        let previous: Option<(i64, i64, String)> = sqlx::query_as(
            "SELECT id,group_id,body FROM social_group_messages
             WHERE sender_id=$1 AND command_key=$2",
        )
        .bind(actor)
        .bind(key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if let Some((id, original_group, original_body)) = previous {
            if original_group == group_id && original_body == body {
                tx.commit().await.map_err(unavailable)?;
                return Ok(SentMessage { id, replayed: true });
            }
            return Err(MailError::Conflict);
        }
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO social_group_messages(group_id,sender_id,body,command_key)
             VALUES($1,$2,$3,$4) RETURNING id",
        )
        .bind(group_id)
        .bind(actor)
        .bind(&body)
        .bind(key)
        .fetch_one(&mut *tx)
        .await
        .map_err(missing_or_unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        Ok(SentMessage {
            id,
            replayed: false,
        })
    }

    /// A group member may depart. No rejoin or creator ownership transfer
    /// contract exists yet; creator exit is refused rather than orphaning it.
    pub async fn leave(&self, actor: i64, group_id: i64) -> Result<(), MailError> {
        if actor <= 0 || group_id <= 0 {
            return Err(MailError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let status: Option<(String, String)> = sqlx::query_as(
            "SELECT status,role FROM social_group_members
             WHERE group_id=$1 AND user_id=$2 FOR UPDATE",
        )
        .bind(group_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        match status {
            Some((status, role)) if status == "active" && role == "creator" => {
                return Err(MailError::Conflict);
            }
            Some((status, _)) if status == "active" => {}
            _ => return Err(MailError::NotFound),
        }
        sqlx::query(
            "UPDATE social_group_members
             SET status='left',left_at=clock_timestamp()
             WHERE group_id=$1 AND user_id=$2 AND status='active'",
        )
        .bind(group_id)
        .bind(actor)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)
    }
}
