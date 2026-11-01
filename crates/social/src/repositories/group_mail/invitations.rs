//! Explicit group invitations: an invitation never grants reading rights.
use super::{unavailable, PgGroupMail};
use crate::domain::MailError;

pub struct PendingGroupInvite {
    pub id: i64,
    pub group_id: i64,
    pub title: String,
    pub inviter_name: String,
}

impl PgGroupMail {
    pub async fn pending_for(&self, actor: i64) -> Result<Vec<PendingGroupInvite>, MailError> {
        if actor <= 0 {
            return Err(MailError::Invalid);
        }
        let rows: Vec<(i64, i64, String, String)> = sqlx::query_as(
            "SELECT i.id,g.id,g.title,u.display_name
             FROM social_group_invitations i
             JOIN social_group_conversations g ON g.id=i.group_id
             JOIN users u ON u.id=i.invited_by
             WHERE i.invited_user_id=$1 AND i.state='pending'
               AND i.expires_at>clock_timestamp()
             ORDER BY i.id DESC LIMIT 50",
        )
        .bind(actor)
        .fetch_all(&self.pool)
        .await
        .map_err(unavailable)?;
        Ok(rows
            .into_iter()
            .map(|(id, group_id, title, inviter_name)| PendingGroupInvite {
                id, group_id, title, inviter_name,
            })
            .collect())
    }

    pub async fn pending_sent(
        &self,
        actor: i64,
        group_id: i64,
    ) -> Result<Vec<(i64, String)>, MailError> {
        if actor <= 0 || group_id <= 0 {
            return Err(MailError::Invalid);
        }
        sqlx::query_as(
            "SELECT i.id,u.display_name FROM social_group_invitations i
             JOIN social_group_members m ON m.group_id=i.group_id
               AND m.user_id=$1 AND m.role='creator' AND m.status='active'
             JOIN users u ON u.id=i.invited_user_id
             WHERE i.group_id=$2 AND i.state='pending'
               AND i.expires_at>clock_timestamp()
             ORDER BY i.id DESC LIMIT 32",
        )
        .bind(actor)
        .bind(group_id)
        .fetch_all(&self.pool)
        .await
        .map_err(unavailable)
    }

    /// Only a live group creator may invite. Former members stay out until
    /// they accept their own new invitation. Never silently restore membership.
    pub async fn invite(
        &self,
        actor: i64,
        group_id: i64,
        recipient: i64,
    ) -> Result<i64, MailError> {
        if actor <= 0 || group_id <= 0 || recipient <= 0 || actor == recipient {
            return Err(MailError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let group: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM social_group_conversations WHERE id=$1 FOR UPDATE",
        )
        .bind(group_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if group.is_none() {
            return Err(MailError::NotFound);
        }
        let creator: Option<i64> = sqlx::query_scalar(
            "SELECT user_id FROM social_group_members
             WHERE group_id=$1 AND user_id=$2 AND status='active'
               AND role='creator' FOR SHARE",
        )
        .bind(group_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if creator.is_none() {
            return Err(MailError::NotFound);
        }
        let account: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM users WHERE id=$1 AND account_state='active'",
        )
        .bind(recipient)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if account.is_none() {
            return Err(MailError::NotFound);
        }
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT status FROM social_group_members
             WHERE group_id=$1 AND user_id=$2 FOR UPDATE",
        )
        .bind(group_id)
        .bind(recipient)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if matches!(existing.as_deref(), Some("active") | Some("banned")) {
            return Err(MailError::Conflict);
        }
        sqlx::query(
            "UPDATE social_group_invitations
             SET state='expired',resolved_at=clock_timestamp()
             WHERE group_id=$1 AND invited_user_id=$2
               AND state='pending' AND expires_at<=clock_timestamp()",
        )
        .bind(group_id)
        .bind(recipient)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        let members: Vec<i64> = sqlx::query_scalar(
            "SELECT user_id FROM social_group_members
             WHERE group_id=$1 AND status='active' ORDER BY user_id",
        )
        .bind(group_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(unavailable)?;
        let pending: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM social_group_invitations
             WHERE group_id=$1 AND state='pending'
               AND expires_at>clock_timestamp()",
        )
        .bind(group_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        if members.len() as i64 + pending >= 32 {
            return Err(MailError::Conflict);
        }
        for &member in &members {
            super::super::mail::pair_lock(&mut tx, member, recipient).await?;
        }
        let blocked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM social_user_blocks
             WHERE (blocker_id=$1 AND blocked_id=ANY($2::bigint[]))
                OR (blocked_id=$1 AND blocker_id=ANY($2::bigint[])))",
        )
        .bind(recipient)
        .bind(&members)
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        if blocked {
            return Err(MailError::Blocked);
        }
        let id: Option<i64> = sqlx::query_scalar(
            "INSERT INTO social_group_invitations(group_id,invited_user_id,invited_by)
             VALUES($1,$2,$3)
             ON CONFLICT (group_id,invited_user_id) WHERE state='pending'
             DO NOTHING RETURNING id",
        )
        .bind(group_id)
        .bind(recipient)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        let id = id.ok_or(MailError::Conflict)?;
        tx.commit().await.map_err(unavailable)?;
        Ok(id)
    }

    /// Only the named recipient can accept. The group lock serializes acceptance
    /// with creator transfer, invitation issuance and administrative revocation.
    pub async fn accept_invite(&self, actor: i64, invite_id: i64) -> Result<i64, MailError> {
        if actor <= 0 || invite_id <= 0 {
            return Err(MailError::Invalid);
        }
        let group_id: Option<i64> = sqlx::query_scalar(
            "SELECT group_id FROM social_group_invitations
             WHERE id=$1 AND invited_user_id=$2 AND state='pending'
               AND expires_at>clock_timestamp()",
        )
        .bind(invite_id)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(unavailable)?;
        let group_id = group_id.ok_or(MailError::NotFound)?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let group: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM social_group_conversations WHERE id=$1 FOR UPDATE",
        )
        .bind(group_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if group.is_none() {
            return Err(MailError::NotFound);
        }
        let live_invitation: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM social_group_invitations
             WHERE id=$1 AND group_id=$2 AND invited_user_id=$3
               AND state='pending' AND expires_at>clock_timestamp()
             FOR UPDATE",
        )
        .bind(invite_id)
        .bind(group_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if live_invitation.is_none() {
            return Err(MailError::NotFound);
        }
        let account: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM users WHERE id=$1 AND account_state='active'",
        )
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if account.is_none() {
            return Err(MailError::NotFound);
        }
        let status: Option<String> = sqlx::query_scalar(
            "SELECT status FROM social_group_members
             WHERE group_id=$1 AND user_id=$2 FOR UPDATE",
        )
        .bind(group_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if matches!(status.as_deref(), Some("active") | Some("banned")) {
            return Err(MailError::Conflict);
        }
        let members: Vec<i64> = sqlx::query_scalar(
            "SELECT user_id FROM social_group_members
             WHERE group_id=$1 AND status='active' ORDER BY user_id",
        )
        .bind(group_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(unavailable)?;
        if members.len() >= 32 {
            return Err(MailError::Conflict);
        }
        for &member in &members {
            super::super::mail::pair_lock(&mut tx, member, actor).await?;
        }
        let blocked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM social_user_blocks
             WHERE (blocker_id=$1 AND blocked_id=ANY($2::bigint[]))
                OR (blocked_id=$1 AND blocker_id=ANY($2::bigint[])))",
        )
        .bind(actor)
        .bind(&members)
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        if blocked {
            return Err(MailError::Blocked);
        }
        let added = sqlx::query(
            "INSERT INTO social_group_members(group_id,user_id,role,status)
             VALUES($1,$2,'member','active')
             ON CONFLICT (group_id,user_id) DO UPDATE SET
               role='member',status='active',joined_at=clock_timestamp(),left_at=NULL
             WHERE social_group_members.status IN ('left','removed')",
        )
        .bind(group_id)
        .bind(actor)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        if added.rows_affected() != 1 {
            return Err(MailError::Conflict);
        }
        let accepted = sqlx::query(
            "UPDATE social_group_invitations SET state='accepted',
               resolved_at=clock_timestamp()
             WHERE id=$1 AND invited_user_id=$2 AND state='pending'",
        )
        .bind(invite_id)
        .bind(actor)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        if accepted.rows_affected() != 1 {
            return Err(MailError::Conflict);
        }
        tx.commit().await.map_err(unavailable)?;
        Ok(group_id)
    }

    pub async fn decline_invite(&self, actor: i64, invite_id: i64) -> Result<(), MailError> {
        if actor <= 0 || invite_id <= 0 {
            return Err(MailError::Invalid);
        }
        let changed = sqlx::query(
            "UPDATE social_group_invitations SET state='declined',
               resolved_at=clock_timestamp()
             WHERE id=$1 AND invited_user_id=$2 AND state='pending'",
        )
        .bind(invite_id)
        .bind(actor)
        .execute(&self.pool)
        .await
        .map_err(unavailable)?;
        if changed.rows_affected() != 1 {
            return Err(MailError::NotFound);
        }
        Ok(())
    }

    pub async fn revoke_invite(
        &self,
        actor: i64,
        group_id: i64,
        invite_id: i64,
    ) -> Result<(), MailError> {
        if actor <= 0 || group_id <= 0 || invite_id <= 0 {
            return Err(MailError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let group: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM social_group_conversations WHERE id=$1 FOR UPDATE",
        )
        .bind(group_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if group.is_none() {
            return Err(MailError::NotFound);
        }
        let creator: Option<i64> = sqlx::query_scalar(
            "SELECT user_id FROM social_group_members
             WHERE group_id=$1 AND user_id=$2 AND role='creator'
               AND status='active' FOR SHARE",
        )
        .bind(group_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if creator.is_none() {
            return Err(MailError::NotFound);
        }
        let changed = sqlx::query(
            "UPDATE social_group_invitations SET state='revoked',
               resolved_at=clock_timestamp()
             WHERE id=$1 AND group_id=$2 AND state='pending'",
        )
        .bind(invite_id)
        .bind(group_id)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        if changed.rows_affected() != 1 {
            return Err(MailError::NotFound);
        }
        tx.commit().await.map_err(unavailable)
    }
}
