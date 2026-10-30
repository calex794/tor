//! E-06 friendship mechanics from NEwBOO; shares the E-08 pair-lock with mail.
use super::{
    mail::{blocked, foreign_or_unavailable, pair_lock, unavailable},
    notification_events,
};
use crate::domain::MailError;
use sqlx::PgPool;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FriendState {
    None,
    Outgoing,
    Incoming,
    Friends,
    BlockedByMe,
    BlockedMe,
}

#[derive(Clone)]
pub struct PgFriendships {
    pool: PgPool,
}

impl PgFriendships {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn check(actor: i64, peer: i64) -> Result<(), MailError> {
        if actor <= 0 || peer <= 0 || actor == peer {
            return Err(MailError::Invalid);
        }
        Ok(())
    }

    /// Membership is the verified session actor, never a user id supplied by a form.
    pub async fn state(&self, actor: i64, peer: i64) -> Result<FriendState, MailError> {
        Self::check(actor, peer)?;
        let my_block: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM social_user_blocks WHERE blocker_id=$1 AND blocked_id=$2)",
        )
        .bind(actor)
        .bind(peer)
        .fetch_one(&self.pool)
        .await
        .map_err(unavailable)?;
        if my_block {
            return Ok(FriendState::BlockedByMe);
        }
        let their_block: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM social_user_blocks WHERE blocker_id=$2 AND blocked_id=$1)",
        )
        .bind(actor)
        .bind(peer)
        .fetch_one(&self.pool)
        .await
        .map_err(unavailable)?;
        if their_block {
            return Ok(FriendState::BlockedMe);
        }
        let row = sqlx::query_as::<_, (String, i64)>(
            "SELECT status,requester_id FROM social_friendships WHERE user_low=$1 AND user_high=$2",
        )
        .bind(actor.min(peer))
        .bind(actor.max(peer))
        .fetch_optional(&self.pool)
        .await
        .map_err(unavailable)?;
        Ok(match row {
            Some((state, _)) if state == "accepted" => FriendState::Friends,
            Some((state, requester)) if state == "pending" && requester == actor => {
                FriendState::Outgoing
            }
            Some((state, _)) if state == "pending" => FriendState::Incoming,
            _ => FriendState::None,
        })
    }

    /// Outgoing request remains pending; a cross-request accepts it.
    /// All writers for this pair serialize through the same lock as mail send/block.
    pub async fn request(&self, actor: i64, peer: i64) -> Result<FriendState, MailError> {
        Self::check(actor, peer)?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        pair_lock(&mut tx, actor, peer).await?;
        if blocked(&mut tx, actor, peer).await? {
            return Err(MailError::Blocked);
        }
        let low = actor.min(peer);
        let high = actor.max(peer);
        let row = sqlx::query_as::<_, (String, i64)>(
            "SELECT status,requester_id FROM social_friendships WHERE user_low=$1 AND user_high=$2",
        )
        .bind(low)
        .bind(high)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        let mut event = None;
        let result = match row {
            None => {
                event = Some(("social.friendship.requested", peer));
                sqlx::query("INSERT INTO social_friendships(user_low,user_high,requester_id,status,action_user_id) VALUES($1,$2,$3,'pending',$3)")
                    .bind(low).bind(high).bind(actor).execute(&mut *tx).await.map_err(foreign_or_unavailable)?;
                FriendState::Outgoing
            }
            Some((status, _)) if status == "accepted" => FriendState::Friends,
            Some((_, requester)) if requester == actor => FriendState::Outgoing,
            Some(_) => {
                event = Some(("social.friendship.accepted", peer));
                sqlx::query("UPDATE social_friendships SET status='accepted',action_user_id=$3,accepted_at=clock_timestamp(),updated_at=clock_timestamp() WHERE user_low=$1 AND user_high=$2")
                    .bind(low).bind(high).bind(actor).execute(&mut *tx).await.map_err(unavailable)?;
                FriendState::Friends
            }
        };
        if let Some((kind, recipient)) = event {
            notification_events::queue(
                &mut tx,
                kind,
                "social_friendship",
                low,
                actor,
                recipient,
                actor,
                None,
            )
            .await?;
        }
        tx.commit().await.map_err(unavailable)?;
        Ok(result)
    }

    pub async fn remove(&self, actor: i64, peer: i64) -> Result<(), MailError> {
        Self::check(actor, peer)?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        pair_lock(&mut tx, actor, peer).await?;
        // An independent block is never removed by a friendship action.
        if blocked(&mut tx, actor, peer).await? {
            return Err(MailError::Blocked);
        }
        let was_friends: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM social_friendships
             WHERE user_low=$1 AND user_high=$2 AND status='accepted')",
        )
        .bind(actor.min(peer))
        .bind(actor.max(peer))
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        sqlx::query("DELETE FROM social_friendships WHERE user_low=$1 AND user_high=$2")
            .bind(actor.min(peer))
            .bind(actor.max(peer))
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        if was_friends {
            notification_events::queue(
                &mut tx,
                "social.friendship.removed",
                "social_friendship",
                actor.min(peer),
                actor,
                peer,
                actor,
                None,
            )
            .await?;
        }
        tx.commit().await.map_err(unavailable)
    }
}
