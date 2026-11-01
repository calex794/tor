//! E-08 administrator-only read views of group and book-room messages.
//! All reads use the existing session+role row locks and commit an audit
//! record before returning data. No membership, impersonation or write grants.
use super::{authorize, unavailable, AdminDirectMessage, PgAdminMail};
use crate::domain::MailError;
use sqlx::{Postgres, Transaction};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminGroupRow {
    pub id: i64,
    pub title: String,
    pub message_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminBookRoomRow {
    pub book_id: i64,
    pub title: String,
    pub message_count: i64,
}

async fn audit_other(
    tx: &mut Transaction<'_, Postgres>,
    admin: i64,
    action: &str,
    group_id: Option<i64>,
    book_id: Option<i64>,
    count: usize,
) -> Result<(), MailError> {
    let count = i32::try_from(count).map_err(|_| MailError::Unavailable)?;
    sqlx::query(
        "INSERT INTO social_mail_admin_access_audit
         (admin_user_id,action,group_id,book_id,result_count)
         VALUES($1,$2,$3,$4,$5)",
    )
    .bind(admin)
    .bind(action)
    .bind(group_id)
    .bind(book_id)
    .bind(count)
    .execute(&mut **tx)
    .await
    .map_err(unavailable)?;
    Ok(())
}

fn valid(before: Option<i64>, limit: u16) -> bool {
    !before.is_some_and(|value| value <= 0) && (1..=50).contains(&limit)
}

impl PgAdminMail {
    /// Search all existing group chats, regardless of administrator membership.
    pub async fn groups(
        &self,
        digest: &[u8; 32],
        before: Option<i64>,
        limit: u16,
    ) -> Result<Vec<AdminGroupRow>, MailError> {
        if !valid(before, limit) {
            return Err(MailError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let admin = authorize(&mut tx, digest).await?;
        let rows = sqlx::query_as::<_, (i64, String, i64)>(
            "SELECT g.id,g.title,
                (SELECT count(*) FROM social_group_messages m WHERE m.group_id=g.id)
             FROM social_group_conversations g
             WHERE ($1::bigint IS NULL OR g.id<$1)
             ORDER BY g.id DESC LIMIT $2",
        )
        .bind(before)
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await
        .map_err(unavailable)?;
        audit_other(&mut tx, admin, "list_group_conversations", None, None, rows.len()).await?;
        tx.commit().await.map_err(unavailable)?;
        Ok(rows
            .into_iter()
            .map(|(id, title, message_count)| AdminGroupRow {
                id,
                title,
                message_count,
            })
            .collect())
    }

    /// Bounded history of any group, including groups the administrator did
    /// not join. A missing chat is not an audited successful read.
    pub async fn group(
        &self,
        digest: &[u8; 32],
        group_id: i64,
        before: Option<i64>,
        limit: u16,
    ) -> Result<Vec<AdminDirectMessage>, MailError> {
        if group_id <= 0 || !valid(before, limit) {
            return Err(MailError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let admin = authorize(&mut tx, digest).await?;
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM social_group_conversations WHERE id=$1",
        )
        .bind(group_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if found.is_none() {
            return Err(MailError::NotFound);
        }
        let mut rows = sqlx::query_as::<_, (i64, i64, String, String)>(
            "SELECT m.id,m.sender_id,u.display_name,m.body
             FROM social_group_messages m JOIN users u ON u.id=m.sender_id
             WHERE m.group_id=$1 AND ($2::bigint IS NULL OR m.id<$2)
             ORDER BY m.id DESC LIMIT $3",
        )
        .bind(group_id)
        .bind(before)
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await
        .map_err(unavailable)?;
        audit_other(
            &mut tx, admin, "read_group_conversation", Some(group_id), None, rows.len(),
        )
        .await?;
        tx.commit().await.map_err(unavailable)?;
        rows.reverse();
        Ok(rows
            .into_iter()
            .map(|(id, sender_id, sender_name, body)| AdminDirectMessage {
                id,
                sender_id,
                sender_name,
                body,
            })
            .collect())
    }

    /// Search canonical Book-room chats. A book may have become a draft or
    /// been put on moderation hold since participants last posted.
    pub async fn book_rooms(
        &self,
        digest: &[u8; 32],
        before: Option<i64>,
        limit: u16,
    ) -> Result<Vec<AdminBookRoomRow>, MailError> {
        if !valid(before, limit) {
            return Err(MailError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let admin = authorize(&mut tx, digest).await?;
        let rows = sqlx::query_as::<_, (i64, String, i64)>(
            "SELECT r.book_id,b.title,
                (SELECT count(*) FROM social_book_room_messages m WHERE m.book_id=r.book_id)
             FROM social_book_rooms r JOIN books b ON b.id=r.book_id
             WHERE ($1::bigint IS NULL OR r.book_id<$1)
             ORDER BY r.book_id DESC LIMIT $2",
        )
        .bind(before)
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await
        .map_err(unavailable)?;
        audit_other(&mut tx, admin, "list_book_rooms", None, None, rows.len()).await?;
        tx.commit().await.map_err(unavailable)?;
        Ok(rows
            .into_iter()
            .map(|(book_id, title, message_count)| AdminBookRoomRow {
                book_id,
                title,
                message_count,
            })
            .collect())
    }

    /// A current administrator may read any existing book-room messages,
    /// even after an author's moderation hold or publication change.
    pub async fn book_room(
        &self,
        digest: &[u8; 32],
        book_id: i64,
        before: Option<i64>,
        limit: u16,
    ) -> Result<Vec<AdminDirectMessage>, MailError> {
        if book_id <= 0 || !valid(before, limit) {
            return Err(MailError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let admin = authorize(&mut tx, digest).await?;
        let found: Option<i64> =
            sqlx::query_scalar("SELECT book_id FROM social_book_rooms WHERE book_id=$1")
                .bind(book_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(unavailable)?;
        if found.is_none() {
            return Err(MailError::NotFound);
        }
        let mut rows = sqlx::query_as::<_, (i64, i64, String, String)>(
            "SELECT m.id,m.sender_id,u.display_name,m.body
             FROM social_book_room_messages m JOIN users u ON u.id=m.sender_id
             WHERE m.book_id=$1 AND ($2::bigint IS NULL OR m.id<$2)
             ORDER BY m.id DESC LIMIT $3",
        )
        .bind(book_id)
        .bind(before)
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await
        .map_err(unavailable)?;
        audit_other(&mut tx, admin, "read_book_room", None, Some(book_id), rows.len()).await?;
        tx.commit().await.map_err(unavailable)?;
        rows.reverse();
        Ok(rows
            .into_iter()
            .map(|(id, sender_id, sender_name, body)| AdminDirectMessage {
                id,
                sender_id,
                sender_name,
                body,
            })
            .collect())
    }
}
