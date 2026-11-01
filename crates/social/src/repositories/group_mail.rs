//! E-08/S057 group-mail storage, isolated from immutable direct-mail pairs.
//! All writes and membership checks are actor-scoped; public pages live in web.
mod invitations;
use super::notification_events::{self, NotificationEvent};
use crate::domain::{clean_body, valid_key, MailError, SentMessage};
pub use invitations::PendingGroupInvite;
use sqlx::PgPool;

#[derive(Clone)]
pub struct PgGroupMail {
    pool: PgPool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GroupConversation {
    pub id: i64,
    pub title: String,
    pub is_creator: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GroupMessage {
    pub id: i64,
    pub sender_id: i64,
    pub body: String,
    pub sender_name: String,
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
        // A group invitation must not bypass a block between any two
        // participants, even when the creator is not one of that pair.
        // Lock every pair in a stable order against concurrent block/unblock.
        for (index, &left) in members.iter().enumerate() {
            for &right in members.iter().skip(index + 1) {
                super::mail::pair_lock(&mut tx, left, right).await?;
            }
        }
        let is_blocked: bool = sqlx::query_scalar(
            "SELECT EXISTS(
               SELECT 1 FROM social_user_blocks b
               WHERE b.blocker_id=ANY($1::bigint[])
                 AND b.blocked_id=ANY($1::bigint[])
             )",
        )
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

    /// Lookup must check active membership before returning even the title.
    pub async fn get(&self, actor: i64, group_id: i64) -> Result<GroupConversation, MailError> {
        if actor <= 0 || group_id <= 0 {
            return Err(MailError::Invalid);
        }
        let row: Option<(i64, String, bool)> = sqlx::query_as(
            "SELECT g.id,g.title,m.role='creator' FROM social_group_conversations g
             JOIN social_group_members m ON m.group_id=g.id
             WHERE g.id=$2 AND m.user_id=$1 AND m.status='active'",
        )
        .bind(actor)
        .bind(group_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(unavailable)?;
        row.map(|(id, title, is_creator)| GroupConversation {
            id,
            title,
            is_creator,
        })
        .ok_or(MailError::NotFound)
    }

    /// Return display names only to a currently active participant. Lock
    /// the actor membership row so leaving cannot race with this disclosure.
    pub async fn members(
        &self,
        actor: i64,
        group_id: i64,
    ) -> Result<Vec<(i64, String)>, MailError> {
        if actor <= 0 || group_id <= 0 {
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
        let rows = sqlx::query_as::<_, (i64, String)>(
            "SELECT m.user_id,u.display_name FROM social_group_members m
             JOIN users u ON u.id=m.user_id
             WHERE m.group_id=$1 AND m.status='active'
             ORDER BY m.user_id LIMIT 32",
        )
        .bind(group_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        Ok(rows)
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
        let rows = sqlx::query_as::<_, (i64, String, bool)>(
            "SELECT g.id,g.title,m.role='creator' FROM social_group_conversations g
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
            .map(|(id, title, is_creator)| GroupConversation {
                id,
                title,
                is_creator,
            })
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
        // Resolve sender names only after the actor membership check;
        // include messages by departed members without revealing a mailbox
        // or exposing account metadata to an outsider.
        let mut rows = sqlx::query_as::<_, (i64, i64, String, String)>(
            "SELECT msg.id,msg.sender_id,msg.body,u.display_name
             FROM social_group_messages msg
             JOIN users u ON u.id=msg.sender_id
             WHERE msg.group_id=$1 AND ($2::bigint IS NULL OR msg.id<$2)
             ORDER BY msg.id DESC LIMIT $3",
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
            .map(|(id, sender_id, body, sender_name)| GroupMessage {
                id,
                sender_id,
                body,
                sender_name,
            })
            .collect())
    }

    /// Ascending after-cursor window so a slow poller cannot skip more
    /// than 50 new messages. Shares the same row-lock ACL as history reads.
    pub async fn messages_after(
        &self,
        actor: i64,
        group_id: i64,
        after: i64,
        limit: u16,
    ) -> Result<Vec<GroupMessage>, MailError> {
        if actor <= 0 || group_id <= 0 || after < 0 || !(1..=50).contains(&limit) {
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
        let rows = sqlx::query_as::<_, (i64, i64, String, String)>(
            "SELECT msg.id,msg.sender_id,msg.body,u.display_name
             FROM social_group_messages msg JOIN users u ON u.id=msg.sender_id
             WHERE msg.group_id=$1 AND msg.id>$2
             ORDER BY msg.id ASC LIMIT $3",
        )
        .bind(group_id)
        .bind(after)
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        Ok(rows
            .into_iter()
            .map(|(id, sender_id, body, sender_name)| GroupMessage {
                id,
                sender_id,
                body,
                sender_name,
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
        // Every active recipient receives a separate immutable source event.
        // Keep it in the message transaction: retries return above and cannot
        // duplicate notifications, even across processes or worker restarts.
        let recipients: Vec<i64> = sqlx::query_scalar(
            "SELECT user_id FROM social_group_members
             WHERE group_id=$1 AND user_id<>$2 AND status='active'
             ORDER BY user_id",
        )
        .bind(group_id)
        .bind(actor)
        .fetch_all(&mut *tx)
        .await
        .map_err(unavailable)?;
        for recipient in recipients {
            let dedupe_key = format!("tor-social:group-message:{id}:recipient:{recipient}");
            notification_events::queue(
                &mut tx,
                NotificationEvent {
                    event_type: "social.group_mail.created",
                    aggregate_kind: "social_group_message",
                    aggregate_id: id,
                    actor,
                    recipient,
                    object_id: group_id,
                    dedupe_key: Some(&dedupe_key),
                },
            )
            .await?;
        }
        tx.commit().await.map_err(unavailable)?;
        Ok(SentMessage {
            id,
            replayed: false,
        })
    }

    /// Transfer the creator role to an existing active member. Lock the group
    /// before the member rows so concurrent transfers cannot create two owners.
    /// A former creator can then leave normally; creation provenance is nullable
    /// when that account is deleted (separate E-08 lifecycle migration).
    pub async fn transfer_creator(
        &self,
        actor: i64,
        group_id: i64,
        next_creator: i64,
    ) -> Result<(), MailError> {
        if actor <= 0 || group_id <= 0 || next_creator <= 0 || actor == next_creator {
            return Err(MailError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let group: Option<i64> =
            sqlx::query_scalar("SELECT id FROM social_group_conversations WHERE id=$1 FOR UPDATE")
                .bind(group_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(unavailable)?;
        if group.is_none() {
            return Err(MailError::NotFound);
        }
        let role: Option<String> = sqlx::query_scalar(
            "SELECT role FROM social_group_members
             WHERE group_id=$1 AND user_id=$2 AND status='active'
             FOR UPDATE",
        )
        .bind(group_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if role.as_deref() != Some("creator") {
            return Err(MailError::NotFound);
        }
        let target: Option<i64> = sqlx::query_scalar(
            "SELECT m.user_id FROM social_group_members m
             JOIN users u ON u.id=m.user_id
             WHERE m.group_id=$1 AND m.user_id=$2 AND m.status='active'
               AND u.account_state='active'
             FOR UPDATE OF m FOR SHARE OF u",
        )
        .bind(group_id)
        .bind(next_creator)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if target.is_none() {
            return Err(MailError::NotFound);
        }
        let creators: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM social_group_members
             WHERE group_id=$1 AND role='creator' AND status='active'",
        )
        .bind(group_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        if creators != 1 {
            return Err(MailError::Conflict);
        }
        let changed = sqlx::query(
            "UPDATE social_group_members
             SET role=CASE WHEN user_id=$3 THEN 'creator' ELSE 'member' END
             WHERE group_id=$1 AND user_id IN ($2,$3) AND status='active'",
        )
        .bind(group_id)
        .bind(actor)
        .bind(next_creator)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        if changed.rows_affected() != 2 {
            return Err(MailError::Conflict);
        }
        // The new creator does not inherit unreviewed outstanding invitations.
        sqlx::query(
            "UPDATE social_group_invitations SET state='revoked',resolved_at=clock_timestamp()
             WHERE group_id=$1 AND state='pending'",
        )
        .bind(group_id)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)
    }

    /// Dissolve only a group whose creator is its sole active member.
    /// The explicit UI form warns that history and invites are deleted.
    pub async fn disband(&self, actor: i64, group_id: i64) -> Result<(), MailError> {
        if actor <= 0 || group_id <= 0 {
            return Err(MailError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let group: Option<i64> =
            sqlx::query_scalar("SELECT id FROM social_group_conversations WHERE id=$1 FOR UPDATE")
                .bind(group_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(unavailable)?;
        if group.is_none() {
            return Err(MailError::NotFound);
        }
        let role: Option<String> = sqlx::query_scalar(
            "SELECT role FROM social_group_members
             WHERE group_id=$1 AND user_id=$2 AND status='active' FOR UPDATE",
        )
        .bind(group_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if role.as_deref() != Some("creator") {
            return Err(MailError::NotFound);
        }
        let active_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM social_group_members
             WHERE group_id=$1 AND status='active'",
        )
        .bind(group_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        if active_count != 1 {
            return Err(MailError::Conflict);
        }
        let result = sqlx::query("DELETE FROM social_group_conversations WHERE id=$1")
            .bind(group_id)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        if result.rows_affected() != 1 {
            return Err(MailError::Conflict);
        }
        tx.commit().await.map_err(unavailable)
    }

    /// A creator must transfer ownership before exiting the group.
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
