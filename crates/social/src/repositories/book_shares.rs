//! S016/S017. Direct book recommendations never grant ownership or chapter access.
use super::mail::{blocked, pair_lock};
use sqlx::{PgPool, Postgres, Row, Transaction};

#[derive(Clone)]
pub struct PgBookShares {
    pool: PgPool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookShareError {
    Invalid,
    NotFound,
    Forbidden,
    RateLimited,
    Conflict,
    Unavailable,
}

impl From<sqlx::Error> for BookShareError {
    fn from(_: sqlx::Error) -> Self {
        Self::Unavailable
    }
}

#[derive(Debug)]
pub struct FriendChoice {
    pub id: i64,
    pub name: String,
}

#[derive(Debug)]
pub struct ReceivedBookShare {
    pub id: i64,
    pub book_id: i64,
    pub book_title: String,
    pub sender_user_id: i64,
    pub sender_name: String,
    pub message: String,
}

impl PgBookShares {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// A share link may be offered only for a currently publicly eligible book.
    pub async fn public_book_title(&self, book_id: i64) -> Result<String, BookShareError> {
        if book_id <= 0 {
            return Err(BookShareError::NotFound);
        }
        let title: Option<String> = sqlx::query_scalar(
            "SELECT title FROM books
             WHERE id=$1 AND visibility='public' AND visibility_status='active'
               AND lifecycle_status='active' AND access_level='basic'
               AND NOT is_deleted AND NOT admin_hold",
        )
        .bind(book_id)
        .fetch_optional(&self.pool)
        .await?;
        title.ok_or(BookShareError::NotFound)
    }

    pub async fn friends(&self, actor: i64) -> Result<Vec<FriendChoice>, BookShareError> {
        if actor <= 0 {
            return Err(BookShareError::Invalid);
        }
        let rows = sqlx::query(
            "SELECT u.id,u.display_name
             FROM social_friendships f
             JOIN users u ON u.id=CASE WHEN f.user_low=$1 THEN f.user_high ELSE f.user_low END
             WHERE (f.user_low=$1 OR f.user_high=$1) AND f.status='accepted'
               AND u.account_state='active'
               AND NOT EXISTS (
                 SELECT 1 FROM social_user_blocks b
                 WHERE (b.blocker_id=$1 AND b.blocked_id=u.id)
                    OR (b.blocker_id=u.id AND b.blocked_id=$1)
               )
               AND NOT EXISTS (
                 SELECT 1 FROM social_book_share_mutes m
                 WHERE m.recipient_user_id=u.id AND m.sender_user_id=$1
               )
             ORDER BY u.display_name,u.id LIMIT 100",
        )
        .bind(actor)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(FriendChoice {
                    id: row.try_get("id")?,
                    name: row.try_get("display_name")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()
            .map_err(Into::into)
    }

    /// A command key is exactly-once within a sender account. A failed request
    /// never creates a record or any entitlement. Friend/block checks share the
    /// E-08 pair lock so block and send cannot race past each other.
    pub async fn send(
        &self,
        sender: i64,
        recipient: i64,
        book_id: i64,
        message: &str,
        command_key: &str,
    ) -> Result<i64, BookShareError> {
        let message = message.trim();
        if sender <= 0
            || recipient <= 0
            || sender == recipient
            || book_id <= 0
            || message.chars().count() > 500
            || message.contains('\0')
            || command_key.len() != 43
            || !command_key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(BookShareError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        pair_lock(&mut tx, sender, recipient)
            .await
            .map_err(|_| BookShareError::Unavailable)?;
        // Serialize sends to the same recipient, regardless of sender.
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtextextended('tor-bookshare-recipient:' ||
                $1::BIGINT::TEXT,0))",
        )
        .bind(recipient)
        .execute(&mut *tx)
        .await?;
        // Sender row lock serializes rate limit checks across all their sends.
        let active_sender: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM users WHERE id=$1 AND account_state='active' FOR UPDATE",
        )
        .bind(sender)
        .fetch_optional(&mut *tx)
        .await?;
        if active_sender.is_none() {
            return Err(BookShareError::NotFound);
        }
        let old = sqlx::query(
            "SELECT id,recipient_user_id,book_id,message FROM social_book_shares
             WHERE sender_user_id=$1 AND command_key=$2",
        )
        .bind(sender)
        .bind(command_key)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(row) = old {
            let peer: i64 = row.try_get("recipient_user_id")?;
            let book: i64 = row.try_get("book_id")?;
            let prior_message: String = row.try_get("message")?;
            if peer != recipient || book != book_id || prior_message != message {
                return Err(BookShareError::Conflict);
            }
            let id: i64 = row.try_get("id")?;
            tx.commit().await?;
            return Ok(id);
        }
        let active_recipient: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM users WHERE id=$1 AND account_state='active' FOR SHARE",
        )
        .bind(recipient)
        .fetch_optional(&mut *tx)
        .await?;
        if active_recipient.is_none() {
            return Err(BookShareError::NotFound);
        }
        if blocked(&mut tx, sender, recipient)
            .await
            .map_err(|_| BookShareError::Unavailable)?
        {
            return Err(BookShareError::Forbidden);
        }
        let friend: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM social_friendships
             WHERE user_low=$1 AND user_high=$2 AND status='accepted')",
        )
        .bind(sender.min(recipient))
        .bind(sender.max(recipient))
        .fetch_one(&mut *tx)
        .await?;
        if !friend {
            return Err(BookShareError::Forbidden);
        }
        let muted: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM social_book_share_mutes
             WHERE recipient_user_id=$1 AND sender_user_id=$2)",
        )
        .bind(recipient)
        .bind(sender)
        .fetch_one(&mut *tx)
        .await?;
        if muted {
            return Err(BookShareError::Forbidden);
        }
        // Basic-only is the conservative first step. Do not expose paid,
        // adult, pending_deletion, ordinary_deleted or admin-held books.
        let eligible: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM books
             WHERE id=$1 AND visibility='public' AND visibility_status='active'
               AND lifecycle_status='active' AND access_level='basic'
               AND NOT is_deleted AND NOT admin_hold FOR SHARE",
        )
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await?;
        if eligible.is_none() {
            return Err(BookShareError::NotFound);
        }
        let sent_last_hour: i64 = sqlx::query_scalar(
            "SELECT count(*)::BIGINT FROM social_book_shares
             WHERE sender_user_id=$1 AND created_at > clock_timestamp() - INTERVAL '1 hour'",
        )
        .bind(sender)
        .fetch_one(&mut *tx)
        .await?;
        let received_today: i64 = sqlx::query_scalar(
            "SELECT count(*)::BIGINT FROM social_book_shares
             WHERE recipient_user_id=$1 AND created_at > clock_timestamp() - INTERVAL '1 day'",
        )
        .bind(recipient)
        .fetch_one(&mut *tx)
        .await?;
        if sent_last_hour >= 10 || received_today >= 30 {
            return Err(BookShareError::RateLimited);
        }
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO social_book_shares(sender_user_id,recipient_user_id,book_id,message,command_key)
             VALUES($1,$2,$3,$4,$5) RETURNING id",
        )
        .bind(sender)
        .bind(recipient)
        .bind(book_id)
        .bind(message)
        .bind(command_key)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }

    /// Recipient-only inbox; a share is a link, not an owned or purchased book.
    pub async fn inbox(&self, recipient: i64) -> Result<Vec<ReceivedBookShare>, BookShareError> {
        if recipient <= 0 {
            return Err(BookShareError::Invalid);
        }
        let rows = sqlx::query(
            "SELECT s.id,s.book_id,b.title AS book_title,
                    s.sender_user_id,u.display_name AS sender_name,s.message
             FROM social_book_shares s
             JOIN books b ON b.id=s.book_id
             JOIN users u ON u.id=s.sender_user_id
             WHERE s.recipient_user_id=$1 AND s.state='delivered'
               AND b.visibility='public' AND b.visibility_status='active'
               AND b.lifecycle_status='active' AND b.access_level='basic'
               AND NOT b.is_deleted AND NOT b.admin_hold
               AND NOT EXISTS(
                  SELECT 1 FROM social_user_blocks bl
                  WHERE (bl.blocker_id=$1 AND bl.blocked_id=s.sender_user_id)
                     OR (bl.blocker_id=s.sender_user_id AND bl.blocked_id=$1)
               )
               AND NOT EXISTS(
                  SELECT 1 FROM social_book_share_mutes m
                  WHERE m.recipient_user_id=$1 AND m.sender_user_id=s.sender_user_id
               )
             ORDER BY s.id DESC LIMIT 50",
        )
        .bind(recipient)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(ReceivedBookShare {
                    id: r.try_get("id")?,
                    book_id: r.try_get("book_id")?,
                    book_title: r.try_get("book_title")?,
                    sender_user_id: r.try_get("sender_user_id")?,
                    sender_name: r.try_get("sender_name")?,
                    message: r.try_get("message")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()
            .map_err(Into::into)
    }

    /// Receiver controls each recommendation independently. Reporting creates
    /// an auditable moderation record; mute suppresses this sender for the future.
    pub async fn receiver_action(
        &self,
        recipient: i64,
        share_id: i64,
        action: &str,
        reason: &str,
        details: &str,
    ) -> Result<(), BookShareError> {
        if recipient <= 0
            || share_id <= 0
            || !matches!(action, "hide" | "report" | "mute")
            || details.chars().count() > 500
            || details.contains('\0')
            || (action == "report" && !matches!(reason, "spam" | "abuse" | "other"))
        {
            return Err(BookShareError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        // Read for pair-lock identity, then revalidate under the row lock.
        let sender: i64 = sqlx::query_scalar(
            "SELECT sender_user_id FROM social_book_shares
             WHERE id=$1 AND recipient_user_id=$2",
        )
        .bind(share_id)
        .bind(recipient)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(BookShareError::NotFound)?;
        if action == "mute" {
            pair_lock(&mut tx, recipient, sender)
                .await
                .map_err(|_| BookShareError::Unavailable)?;
        }
        let row = sqlx::query(
            "SELECT sender_user_id,state FROM social_book_shares
             WHERE id=$1 AND recipient_user_id=$2 FOR UPDATE",
        )
        .bind(share_id)
        .bind(recipient)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(BookShareError::NotFound)?;
        let sender: i64 = row.try_get("sender_user_id")?;
        let old: String = row.try_get("state")?;
        if old != "delivered" {
            if old
                == match action {
                    "hide" => "hidden",
                    "report" => "reported",
                    _ => "muted",
                }
            {
                tx.commit().await?;
                return Ok(());
            }
            return Err(BookShareError::Conflict);
        }
        if action == "report" {
            sqlx::query(
                "INSERT INTO social_book_share_reports(share_id,reporter_user_id,reason,details)
                 VALUES($1,$2,$3,$4) ON CONFLICT(share_id) DO NOTHING",
            )
            .bind(share_id)
            .bind(recipient)
            .bind(reason)
            .bind(details.trim())
            .execute(&mut *tx)
            .await?;
        }
        if action == "mute" {
            sqlx::query(
                "INSERT INTO social_book_share_mutes(recipient_user_id,sender_user_id)
                 VALUES($1,$2) ON CONFLICT DO NOTHING",
            )
            .bind(recipient)
            .bind(sender)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE social_book_shares SET state='muted',updated_at=clock_timestamp()
                 WHERE recipient_user_id=$1 AND sender_user_id=$2 AND state='delivered'",
            )
            .bind(recipient)
            .bind(sender)
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query(
                "UPDATE social_book_shares SET state=$2,updated_at=clock_timestamp()
                 WHERE id=$1",
            )
            .bind(share_id)
            .bind(if action == "hide" {
                "hidden"
            } else {
                "reported"
            })
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
