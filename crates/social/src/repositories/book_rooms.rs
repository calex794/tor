//! E-08/S057 book rooms: one canonical public room per book, independent
//! of private groups and direct conversations. Membership is explicit.
//! Private/held/deleted books never yield room content to readers.
use crate::domain::{clean_body, valid_key, MailError, SentMessage};
use sqlx::{PgPool, Postgres, Transaction};

#[derive(Clone)]
pub struct PgBookRooms {
    pool: PgPool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BookRoomMessage {
    pub id: i64,
    pub sender_id: i64,
    pub sender_name: String,
    pub body: String,
}

fn unavailable(_: sqlx::Error) -> MailError {
    MailError::Unavailable
}

fn missing_or_unavailable(error: sqlx::Error) -> MailError {
    if error.as_database_error().and_then(|e| e.code()).as_deref() == Some("23503") {
        MailError::NotFound
    } else {
        MailError::Unavailable
    }
}

/// Lock the canonical book row against a concurrent draft/hold/deletion.
/// This check is performed separately for join, send and read, not only join.
async fn public_book(
    tx: &mut Transaction<'_, Postgres>,
    book_id: i64,
) -> Result<(), MailError> {
    let available: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM books WHERE id=$1 AND visibility='public'
         AND NOT admin_hold AND NOT is_deleted FOR SHARE",
    )
    .bind(book_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(unavailable)?;
    available.map(|_| ()).ok_or(MailError::NotFound)
}

async fn active_membership(
    tx: &mut Transaction<'_, Postgres>,
    actor: i64,
    book_id: i64,
    exclusive: bool,
) -> Result<(), MailError> {
    let query = if exclusive {
        "SELECT status FROM social_book_room_members
         WHERE book_id=$1 AND user_id=$2 FOR UPDATE"
    } else {
        "SELECT status FROM social_book_room_members
         WHERE book_id=$1 AND user_id=$2 FOR SHARE"
    };
    let status: Option<String> = sqlx::query_scalar(query)
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&mut **tx)
        .await
        .map_err(unavailable)?;
    if status.as_deref() != Some("active") {
        return Err(MailError::NotFound);
    }
    Ok(())
}

impl PgBookRooms {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Join or rejoin the one room bound to a public Book ID. An existing
    /// active membership is unchanged. Two simultaneous first joins cannot
    /// create two room identities due to the book_id primary key.
    pub async fn join(&self, actor: i64, book_id: i64) -> Result<(), MailError> {
        if actor <= 0 || book_id <= 0 {
            return Err(MailError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        public_book(&mut tx, book_id).await?;
        let active_actor: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM users WHERE id=$1 AND account_state='active' FOR SHARE",
        )
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if active_actor.is_none() {
            return Err(MailError::NotFound);
        }
        sqlx::query(
            "INSERT INTO social_book_rooms(book_id,created_by)
             VALUES($1,$2) ON CONFLICT(book_id) DO NOTHING",
        )
        .bind(book_id)
        .bind(actor)
        .execute(&mut *tx)
        .await
        .map_err(missing_or_unavailable)?;
        sqlx::query(
            "INSERT INTO social_book_room_members(book_id,user_id)
             VALUES($1,$2)
             ON CONFLICT(book_id,user_id) DO UPDATE
             SET status='active',joined_at=clock_timestamp(),left_at=NULL
             WHERE social_book_room_members.status='left'",
        )
        .bind(book_id)
        .bind(actor)
        .execute(&mut *tx)
        .await
        .map_err(missing_or_unavailable)?;
        tx.commit().await.map_err(unavailable)
    }

    /// Exiting is permitted even if a previously public book becomes hidden.
    /// It never deletes the canonical room or rewrites prior messages.
    pub async fn leave(&self, actor: i64, book_id: i64) -> Result<(), MailError> {
        if actor <= 0 || book_id <= 0 {
            return Err(MailError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        active_membership(&mut tx, actor, book_id, true).await?;
        sqlx::query(
            "UPDATE social_book_room_members
             SET status='left',left_at=clock_timestamp()
             WHERE book_id=$1 AND user_id=$2 AND status='active'",
        )
        .bind(book_id)
        .bind(actor)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)
    }

    /// Reader-visible bounded history. An outsider or departed member cannot
    /// use the book ID to enumerate messages, sender names or book metadata.
    pub async fn messages(
        &self,
        actor: i64,
        book_id: i64,
        before: Option<i64>,
        limit: u16,
    ) -> Result<Vec<BookRoomMessage>, MailError> {
        if actor <= 0
            || book_id <= 0
            || before.is_some_and(|id| id <= 0)
            || !(1..=100).contains(&limit)
        {
            return Err(MailError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        public_book(&mut tx, book_id).await?;
        active_membership(&mut tx, actor, book_id, false).await?;
        let mut rows = sqlx::query_as::<_, (i64, i64, String, String)>(
            "SELECT m.id,m.sender_id,u.display_name,m.body
             FROM social_book_room_messages m
             JOIN users u ON u.id=m.sender_id
             WHERE m.book_id=$1 AND ($2::bigint IS NULL OR m.id<$2)
             ORDER BY m.id DESC LIMIT $3",
        )
        .bind(book_id)
        .bind(before)
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        rows.reverse();
        Ok(rows
            .into_iter()
            .map(|(id, sender_id, sender_name, body)| BookRoomMessage {
                id,
                sender_id,
                sender_name,
                body,
            })
            .collect())
    }

    /// Store a message and its unique retry receipt within the member lock;
    /// no notification or external side effect occurs on replay.
    pub async fn send(
        &self,
        actor: i64,
        book_id: i64,
        body: &str,
        command_key: &str,
    ) -> Result<SentMessage, MailError> {
        if actor <= 0 || book_id <= 0 || !valid_key(command_key) {
            return Err(MailError::Invalid);
        }
        let body = clean_body(body)?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        public_book(&mut tx, book_id).await?;
        active_membership(&mut tx, actor, book_id, true).await?;
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtextextended(
                'tor-social-book-room:' || $1::bigint::text || ':' || $2::text,0))",
        )
        .bind(actor)
        .bind(command_key)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        let previous: Option<(i64, i64, String)> = sqlx::query_as(
            "SELECT id,book_id,body FROM social_book_room_messages
             WHERE sender_id=$1 AND command_key=$2",
        )
        .bind(actor)
        .bind(command_key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if let Some((id, previous_book, previous_body)) = previous {
            if previous_book != book_id || previous_body != body {
                return Err(MailError::Conflict);
            }
            tx.commit().await.map_err(unavailable)?;
            return Ok(SentMessage { id, replayed: true });
        }
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO social_book_room_messages(book_id,sender_id,body,command_key)
             VALUES($1,$2,$3,$4) RETURNING id",
        )
        .bind(book_id)
        .bind(actor)
        .bind(&body)
        .bind(command_key)
        .fetch_one(&mut *tx)
        .await
        .map_err(missing_or_unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        Ok(SentMessage {
            id,
            replayed: false,
        })
    }
}
