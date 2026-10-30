//! E-02 first database slice: public book comments only.
//! This repository never grants reading rights and never exposes chapter comments.
use sqlx::PgPool;
use crate::domain::mentions;

#[derive(Clone)]
pub struct PgPublicBookComments {
    pool: PgPool,
}

#[derive(Debug, Eq, PartialEq)]
pub enum CreateResult {
    Created(i64),
    Replayed(i64),
    Conflict,
    NotFound,
    Unavailable,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PublicComment {
    pub id: i64,
    pub author_user_id: i64,
    pub body: String,
    pub parent_id: Option<i64>,
}

/// A private navigation reference, deliberately without either comment body.
#[derive(Debug, Eq, PartialEq)]
pub struct ReceivedReply {
    pub comment_id: i64,
    pub book_id: i64,
    pub book_title: String,
    pub is_read: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListCommentsError {
    Invalid,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadStateResult {
    Updated,
    NotFound,
    Unavailable,
}

impl PgPublicBookComments {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Called only with a verified session actor. Never take actor from an HTTP body.
    /// Same transaction protects both the target visibility and idempotent mutation.
    pub async fn create(
        &self,
        actor_id: i64,
        book_id: i64,
        body: &str,
        command_key: &str,
        payload_hash: &[u8],
    ) -> CreateResult {
        self.create_with_parent(actor_id, book_id, body, command_key, payload_hash, None)
            .await
    }

    /// Same canonical comment and replay identity, with an optional reply target.
    pub async fn create_with_parent(
        &self,
        actor_id: i64,
        book_id: i64,
        body: &str,
        command_key: &str,
        payload_hash: &[u8],
        parent_id: Option<i64>,
    ) -> CreateResult {
        if actor_id <= 0
            || book_id <= 0
            || parent_id.is_some_and(|id| id <= 0)
            || body.is_empty()
            || body.trim() != body
            || body.chars().count() > 2000
            || body.contains('\0')
            || command_key.len() != 43
            || !command_key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || payload_hash.len() != 32
        {
            return CreateResult::Conflict;
        }
        let mut tx = match self.pool.begin().await {
            Ok(tx) => tx,
            Err(_) => return CreateResult::Unavailable,
        };
        // Lock publication state: a parallel withdrawal cannot race past the
        // comment insert. Hidden/held/deleted objects never permit a new write.
        let allowed = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM books
             WHERE id=$1 AND visibility='public'
               AND NOT admin_hold AND NOT is_deleted FOR SHARE",
        )
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await;
        match allowed {
            Ok(Some(_)) => {}
            Ok(None) => return CreateResult::NotFound,
            Err(_) => return CreateResult::Unavailable,
        }
        // Lock the exact visible parent until commit. A concurrent moderation
        // update must serialize with this write; no copied parent body is stored.
        if let Some(parent_id) = parent_id {
            let parent = sqlx::query_scalar::<_, i64>(
                "SELECT id FROM discussion_comments
                 WHERE id=$1 AND book_id=$2 AND visibility='visible'
                   AND chapter_id IS NULL AND version_id IS NULL FOR SHARE",
            )
            .bind(parent_id)
            .bind(book_id)
            .fetch_optional(&mut *tx)
            .await;
            match parent {
                Ok(Some(_)) => {}
                Ok(None) => return CreateResult::NotFound,
                Err(_) => return CreateResult::Unavailable,
            }
        }
        // Resolve structured IDs solely against visible participants of this book.
        // This prevents arbitrary user-id tokens from creating notifications.
        let tokens = mentions::candidates(body);
        let ids: Vec<i64> = tokens.iter().map(|token| token.user_id).collect();
        let eligible = if ids.is_empty() {
            Vec::new()
        } else {
            match sqlx::query_scalar::<_, i64>(
                "SELECT DISTINCT c.author_user_id FROM discussion_comments c
                 JOIN users u ON u.id=c.author_user_id
                 WHERE c.book_id=$1 AND c.visibility='visible'
                   AND c.chapter_id IS NULL AND c.version_id IS NULL
                   AND c.author_user_id=ANY($2) AND c.author_user_id<>$3
                 ORDER BY c.author_user_id LIMIT 5",
            )
            .bind(book_id)
            .bind(&ids)
            .bind(actor_id)
            .fetch_all(&mut *tx)
            .await {
                Ok(ids) => ids,
                Err(_) => return CreateResult::Unavailable,
            }
        };
        let eligible = eligible.into_iter().take(5).collect::<Vec<_>>();
        let clean_body = mentions::canonicalize(body, &eligible);
                // Serialize attempts with the same actor/key on the same PostgreSQL row.
        // A conflicting key must not silently return a different saved comment.
        let inserted = sqlx::query_scalar::<_, i64>(
            "INSERT INTO discussion_comments
               (author_user_id,book_id,body,command_key,command_hash,visibility,parent_id)
             VALUES ($1,$2,$3,$4,$5,'visible',$6)
             ON CONFLICT (author_user_id,command_key) DO NOTHING
             RETURNING id",
        )
        .bind(actor_id)
        .bind(book_id)
        .bind(&clean_body)
        .bind(command_key)
        .bind(payload_hash)
        .bind(parent_id)
        .fetch_optional(&mut *tx)
        .await;
        let result = match inserted {
            Ok(Some(id)) => CreateResult::Created(id),
            Ok(None) => {
                let original = sqlx::query_as::<_, (i64, i64, Vec<u8>, String, Option<i64>)>(
                    "SELECT id,book_id,command_hash,body,parent_id FROM discussion_comments
                     WHERE author_user_id=$1 AND command_key=$2 AND book_id IS NOT NULL",
                )
                .bind(actor_id)
                .bind(command_key)
                .fetch_optional(&mut *tx)
                .await;
                match original {
                    Ok(Some((id, original_book, hash, old_body, old_parent)))
                        if original_book == book_id
                            && hash == payload_hash
                            && old_body == clean_body
                            && old_parent == parent_id =>
                    {
                        CreateResult::Replayed(id)
                    }
                    Ok(_) => CreateResult::Conflict,
                    Err(_) => CreateResult::Unavailable,
                }
            }
            Err(_) => CreateResult::Unavailable,
        };
        if let CreateResult::Created(id) | CreateResult::Replayed(id) = result {
            for recipient in &eligible {
                if sqlx::query(
                    "INSERT INTO discussion_comment_mentions(comment_id,recipient_user_id)
                     VALUES ($1,$2) ON CONFLICT DO NOTHING",
                )
                .bind(id)
                .bind(recipient)
                .execute(&mut *tx)
                .await
                .is_err() {
                    return CreateResult::Unavailable;
                }
            }
            if tx.commit().await.is_err() {
                return CreateResult::Unavailable;
            }
        }
        result
    }

    /// Derive recipients from canonical parents, never from browser input or
    /// copied notification text. Recheck both comments and the book in one query.
    pub async fn received_replies(
        &self,
        actor_id: i64,
        limit: u16,
    ) -> Result<Vec<ReceivedReply>, ListCommentsError> {
        if actor_id <= 0 || limit == 0 || limit > 100 {
            return Err(ListCommentsError::Invalid);
        }
        let rows = sqlx::query_as::<_, (i64, i64, String, bool)>(
            "SELECT reply.id, b.id, b.title, (seen.reply_comment_id IS NOT NULL)
             FROM discussion_comments reply
             JOIN discussion_comments parent ON parent.id=reply.parent_id
             JOIN books b ON b.id=reply.book_id
             LEFT JOIN discussion_reply_reads seen
               ON seen.recipient_user_id=$1 AND seen.reply_comment_id=reply.id
             WHERE parent.author_user_id=$1 AND reply.author_user_id<>$1
               AND parent.book_id=reply.book_id
               AND parent.chapter_id IS NULL AND reply.chapter_id IS NULL
               AND parent.version_id IS NULL AND reply.version_id IS NULL
               AND parent.visibility='visible' AND reply.visibility='visible'
               AND b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
             ORDER BY reply.id DESC LIMIT $2",
        )
        .bind(actor_id)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(|_| ListCommentsError::Unavailable)?;
        Ok(rows
            .into_iter()
            .map(|(comment_id, book_id, book_title, is_read)| ReceivedReply {
                comment_id,
                book_id,
                book_title,
                is_read,
            })
            .collect())
    }

    /// Update a single received reply, never an arbitrary comment or a client-supplied recipient.
    /// The parent, reply, and public book remain visible and locked until commit.
    pub async fn set_reply_read_state(
        &self,
        actor_id: i64,
        reply_id: i64,
        read: bool,
    ) -> ReadStateResult {
        if actor_id <= 0 || reply_id <= 0 {
            return ReadStateResult::NotFound;
        }
        let mut tx = match self.pool.begin().await {
            Ok(tx) => tx,
            Err(_) => return ReadStateResult::Unavailable,
        };
        let allowed = sqlx::query_scalar::<_, i64>(
            "SELECT reply.id
             FROM discussion_comments reply
             JOIN discussion_comments parent ON parent.id=reply.parent_id
             JOIN books b ON b.id=reply.book_id
             WHERE reply.id=$2 AND parent.author_user_id=$1
               AND reply.author_user_id<>$1 AND parent.book_id=reply.book_id
               AND parent.chapter_id IS NULL AND reply.chapter_id IS NULL
               AND parent.version_id IS NULL AND reply.version_id IS NULL
               AND parent.visibility='visible' AND reply.visibility='visible'
               AND b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
             FOR SHARE OF reply, parent, b",
        )
        .bind(actor_id)
        .bind(reply_id)
        .fetch_optional(&mut *tx)
        .await;
        match allowed {
            Ok(Some(_)) => {}
            Ok(None) => return ReadStateResult::NotFound,
            Err(_) => return ReadStateResult::Unavailable,
        }
        let changed = if read {
            sqlx::query(
                "INSERT INTO discussion_reply_reads(recipient_user_id,reply_comment_id)
                 VALUES($1,$2) ON CONFLICT (recipient_user_id,reply_comment_id) DO NOTHING",
            )
            .bind(actor_id)
            .bind(reply_id)
            .execute(&mut *tx)
            .await
        } else {
            sqlx::query(
                "DELETE FROM discussion_reply_reads
                 WHERE recipient_user_id=$1 AND reply_comment_id=$2",
            )
            .bind(actor_id)
            .bind(reply_id)
            .execute(&mut *tx)
            .await
        };
        if changed.is_err() || tx.commit().await.is_err() {
            return ReadStateResult::Unavailable;
        }
        ReadStateResult::Updated
    }

    /// Only public/visible book comments; all rights are rechecked on every GET.
    pub async fn list(
        &self,
        book_id: i64,
        after_id: Option<i64>,
        limit: u16,
    ) -> Result<Option<Vec<PublicComment>>, ListCommentsError> {
        if book_id <= 0 || limit == 0 || limit > 100 || after_id.is_some_and(|n| n <= 0) {
            return Err(ListCommentsError::Invalid);
        }
        let visible = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM books WHERE id=$1
               AND visibility='public' AND NOT admin_hold AND NOT is_deleted",
        )
        .bind(book_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| ListCommentsError::Unavailable)?;
        if visible.is_none() {
            return Ok(None);
        }
        // The visibility join inside this query avoids a time-of-check/time-of-use leak.
        let rows = sqlx::query_as::<_, (i64, i64, String, Option<i64>)>(
            "SELECT c.id,c.author_user_id,c.body,c.parent_id
             FROM discussion_comments c JOIN books b ON b.id=c.book_id
             WHERE b.id=$1 AND b.visibility='public'
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND c.visibility='visible'
               AND ($2::BIGINT IS NULL OR c.id < $2)
             ORDER BY c.id DESC LIMIT $3",
        )
        .bind(book_id)
        .bind(after_id)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(|_| ListCommentsError::Unavailable)?;
        Ok(Some(
            rows.into_iter()
                .map(|(id, author_user_id, body, parent_id)| PublicComment {
                    id,
                    author_user_id,
                    body,
                    parent_id,
                })
                .collect(),
        ))
    }
}
