//! Consent-based creator handoff. A proposal never changes group ownership.
use super::{unavailable, PgGroupMail};
use crate::domain::MailError;

pub struct PendingGroupTransfer {
    pub id: i64,
    pub group_id: i64,
    pub title: String,
    pub other_name: String,
}

impl PgGroupMail {
    pub async fn pending_transfer_incoming(
        &self,
        actor: i64,
    ) -> Result<Vec<PendingGroupTransfer>, MailError> {
        if actor <= 0 {
            return Err(MailError::Invalid);
        }
        let rows: Vec<(i64, i64, String, String)> = sqlx::query_as(
            "SELECT h.id,g.id,g.title,u.display_name
             FROM social_group_creator_transfers h
             JOIN social_group_conversations g ON g.id=h.group_id
             JOIN social_group_members m ON m.group_id=h.group_id
               AND m.user_id=$1 AND m.status='active'
             JOIN social_group_members owner ON owner.group_id=h.group_id
               AND owner.user_id=h.from_user_id AND owner.status='active'
               AND owner.role='creator'
             JOIN users u ON u.id=h.from_user_id AND u.account_state='active'
             WHERE h.to_user_id=$1 AND h.state='pending'
               AND h.expires_at>clock_timestamp()
             ORDER BY h.id DESC LIMIT 50",
        )
        .bind(actor)
        .fetch_all(&self.pool)
        .await
        .map_err(unavailable)?;
        Ok(rows
            .into_iter()
            .map(|(id, group_id, title, other_name)| PendingGroupTransfer {
                id,
                group_id,
                title,
                other_name,
            })
            .collect())
    }

    pub async fn pending_transfer_outgoing(
        &self,
        actor: i64,
    ) -> Result<Vec<PendingGroupTransfer>, MailError> {
        if actor <= 0 {
            return Err(MailError::Invalid);
        }
        let rows: Vec<(i64, i64, String, String)> = sqlx::query_as(
            "SELECT h.id,g.id,g.title,u.display_name
             FROM social_group_creator_transfers h
             JOIN social_group_conversations g ON g.id=h.group_id
             JOIN social_group_members owner ON owner.group_id=h.group_id
               AND owner.user_id=$1 AND owner.status='active'
               AND owner.role='creator'
             JOIN users u ON u.id=h.to_user_id
             WHERE h.from_user_id=$1 AND h.state='pending'
               AND h.expires_at>clock_timestamp()
             ORDER BY h.id DESC LIMIT 50",
        )
        .bind(actor)
        .fetch_all(&self.pool)
        .await
        .map_err(unavailable)?;
        Ok(rows
            .into_iter()
            .map(|(id, group_id, title, other_name)| PendingGroupTransfer {
                id,
                group_id,
                title,
                other_name,
            })
            .collect())
    }

    /// Propose, but never grant, management rights. A group has at most one
    /// unexpired outstanding proposal at any instant.
    pub async fn propose_transfer(
        &self,
        actor: i64,
        group_id: i64,
        recipient: i64,
    ) -> Result<i64, MailError> {
        if actor <= 0 || group_id <= 0 || recipient <= 0 || actor == recipient {
            return Err(MailError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let found: Option<i64> =
            sqlx::query_scalar("SELECT id FROM social_group_conversations WHERE id=$1 FOR UPDATE")
                .bind(group_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(unavailable)?;
        if found.is_none() {
            return Err(MailError::NotFound);
        }
        let creator: Option<i64> = sqlx::query_scalar(
            "SELECT m.user_id FROM social_group_members m
             JOIN users u ON u.id=m.user_id AND u.account_state='active'
             WHERE m.group_id=$1 AND m.user_id=$2 AND m.role='creator'
               AND m.status='active' FOR UPDATE OF m FOR SHARE OF u",
        )
        .bind(group_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if creator.is_none() {
            return Err(MailError::NotFound);
        }
        let candidate: Option<i64> = sqlx::query_scalar(
            "SELECT m.user_id FROM social_group_members m
             JOIN users u ON u.id=m.user_id AND u.account_state='active'
             WHERE m.group_id=$1 AND m.user_id=$2 AND m.status='active'
             FOR UPDATE OF m FOR SHARE OF u",
        )
        .bind(group_id)
        .bind(recipient)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if candidate.is_none() {
            return Err(MailError::NotFound);
        }
        let owners: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM social_group_members
             WHERE group_id=$1 AND role='creator' AND status='active'",
        )
        .bind(group_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        if owners != 1 {
            return Err(MailError::Conflict);
        }
        sqlx::query(
            "UPDATE social_group_creator_transfers
             SET state='expired',resolved_at=clock_timestamp()
             WHERE group_id=$1 AND state='pending'
               AND expires_at<=clock_timestamp()",
        )
        .bind(group_id)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        let inserted: Option<i64> = sqlx::query_scalar(
            "INSERT INTO social_group_creator_transfers(group_id,from_user_id,to_user_id)
             VALUES($1,$2,$3)
             ON CONFLICT (group_id) WHERE state='pending'
             DO NOTHING RETURNING id",
        )
        .bind(group_id)
        .bind(actor)
        .bind(recipient)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        let id = inserted.ok_or(MailError::Conflict)?;
        tx.commit().await.map_err(unavailable)?;
        Ok(id)
    }

    /// Only the proposed successor can accept; no direct or owner-only transfer
    /// endpoint remains. Membership and active accounts are rechecked inside
    /// the same transaction that changes both roles.
    pub async fn accept_transfer(&self, actor: i64, offer_id: i64) -> Result<i64, MailError> {
        if actor <= 0 || offer_id <= 0 {
            return Err(MailError::Invalid);
        }
        let group_id: Option<i64> = sqlx::query_scalar(
            "SELECT group_id FROM social_group_creator_transfers
             WHERE id=$1 AND to_user_id=$2 AND state='pending'
               AND expires_at>clock_timestamp()",
        )
        .bind(offer_id)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(unavailable)?;
        let group_id = group_id.ok_or(MailError::NotFound)?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let found: Option<i64> =
            sqlx::query_scalar("SELECT id FROM social_group_conversations WHERE id=$1 FOR UPDATE")
                .bind(group_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(unavailable)?;
        if found.is_none() {
            return Err(MailError::NotFound);
        }
        let from_user: Option<i64> = sqlx::query_scalar(
            "SELECT from_user_id FROM social_group_creator_transfers
             WHERE id=$1 AND group_id=$2 AND to_user_id=$3
               AND state='pending' AND expires_at>clock_timestamp() FOR UPDATE",
        )
        .bind(offer_id)
        .bind(group_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        let from_user = from_user.ok_or(MailError::NotFound)?;
        let creator: Option<i64> = sqlx::query_scalar(
            "SELECT m.user_id FROM social_group_members m
             JOIN users u ON u.id=m.user_id AND u.account_state='active'
             WHERE m.group_id=$1 AND m.user_id=$2 AND m.status='active'
               AND m.role='creator' FOR UPDATE OF m FOR SHARE OF u",
        )
        .bind(group_id)
        .bind(from_user)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if creator.is_none() {
            return Err(MailError::NotFound);
        }
        let recipient: Option<i64> = sqlx::query_scalar(
            "SELECT m.user_id FROM social_group_members m
             JOIN users u ON u.id=m.user_id AND u.account_state='active'
             WHERE m.group_id=$1 AND m.user_id=$2 AND m.status='active'
             FOR UPDATE OF m FOR SHARE OF u",
        )
        .bind(group_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if recipient.is_none() {
            return Err(MailError::NotFound);
        }
        let owners: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM social_group_members
             WHERE group_id=$1 AND role='creator' AND status='active'",
        )
        .bind(group_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        if owners != 1 {
            return Err(MailError::Conflict);
        }
        let updated = sqlx::query(
            "UPDATE social_group_members
             SET role=CASE WHEN user_id=$3 THEN 'creator' ELSE 'member' END
             WHERE group_id=$1 AND user_id IN ($2,$3) AND status='active'",
        )
        .bind(group_id)
        .bind(from_user)
        .bind(actor)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        if updated.rows_affected() != 2 {
            return Err(MailError::Conflict);
        }
        // Do not pass the old creator's unreviewed invitations to the new one.
        sqlx::query(
            "UPDATE social_group_invitations
             SET state='revoked',resolved_at=clock_timestamp()
             WHERE group_id=$1 AND state='pending'",
        )
        .bind(group_id)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        let accepted = sqlx::query(
            "UPDATE social_group_creator_transfers
             SET state='accepted',resolved_at=clock_timestamp()
             WHERE id=$1 AND state='pending'",
        )
        .bind(offer_id)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        if accepted.rows_affected() != 1 {
            return Err(MailError::Conflict);
        }
        tx.commit().await.map_err(unavailable)?;
        Ok(group_id)
    }

    pub async fn decline_transfer(&self, actor: i64, offer_id: i64) -> Result<(), MailError> {
        if actor <= 0 || offer_id <= 0 {
            return Err(MailError::Invalid);
        }
        let group_id: Option<i64> = sqlx::query_scalar(
            "SELECT group_id FROM social_group_creator_transfers
             WHERE id=$1 AND to_user_id=$2 AND state='pending'",
        )
        .bind(offer_id)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(unavailable)?;
        let group_id = group_id.ok_or(MailError::NotFound)?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let found: Option<i64> =
            sqlx::query_scalar("SELECT id FROM social_group_conversations WHERE id=$1 FOR UPDATE")
                .bind(group_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(unavailable)?;
        if found.is_none() {
            return Err(MailError::NotFound);
        }
        let changed = sqlx::query(
            "UPDATE social_group_creator_transfers
             SET state='declined',resolved_at=clock_timestamp()
             WHERE id=$1 AND group_id=$2 AND to_user_id=$3
               AND state='pending' AND expires_at>clock_timestamp()",
        )
        .bind(offer_id)
        .bind(group_id)
        .bind(actor)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        if changed.rows_affected() != 1 {
            return Err(MailError::NotFound);
        }
        tx.commit().await.map_err(unavailable)
    }

    pub async fn revoke_transfer(
        &self,
        actor: i64,
        group_id: i64,
        offer_id: i64,
    ) -> Result<(), MailError> {
        if actor <= 0 || group_id <= 0 || offer_id <= 0 {
            return Err(MailError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let found: Option<i64> =
            sqlx::query_scalar("SELECT id FROM social_group_conversations WHERE id=$1 FOR UPDATE")
                .bind(group_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(unavailable)?;
        if found.is_none() {
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
            "UPDATE social_group_creator_transfers
             SET state='revoked',resolved_at=clock_timestamp()
             WHERE id=$1 AND group_id=$2 AND from_user_id=$3
               AND state='pending' AND expires_at>clock_timestamp()",
        )
        .bind(offer_id)
        .bind(group_id)
        .bind(actor)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        if changed.rows_affected() != 1 {
            return Err(MailError::NotFound);
        }
        tx.commit().await.map_err(unavailable)
    }
}
