//! E-02 first database slice: public book comments only.
//! This repository never grants reading rights and never exposes chapter comments.
use sqlx::PgPool;

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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListCommentsError {
    Invalid,
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
        if actor_id <= 0
            || book_id <= 0
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
        // Serialize attempts with the same actor/key on the same PostgreSQL row.
        // A conflicting key must not silently return a different saved comment.
        let inserted = sqlx::query_scalar::<_, i64>(
            "INSERT INTO discussion_comments
               (author_user_id,book_id,body,command_key,command_hash,visibility)
             VALUES ($1,$2,$3,$4,$5,'visible')
             ON CONFLICT (author_user_id,command_key) DO NOTHING
             RETURNING id",
        )
        .bind(actor_id)
        .bind(book_id)
        .bind(body)
        .bind(command_key)
        .bind(payload_hash)
        .fetch_optional(&mut *tx)
        .await;
        let result = match inserted {
            Ok(Some(id)) => CreateResult::Created(id),
            Ok(None) => {
                let original = sqlx::query_as::<_, (i64, i64, Vec<u8>, String)>(
                    "SELECT id,book_id,command_hash,body FROM discussion_comments
                     WHERE author_user_id=$1 AND command_key=$2 AND book_id IS NOT NULL",
                )
                .bind(actor_id)
                .bind(command_key)
                .fetch_optional(&mut *tx)
                .await;
                match original {
                    Ok(Some((id, original_book, hash, old_body)))
                        if original_book == book_id && hash == payload_hash && old_body == body =>
                    {
                        CreateResult::Replayed(id)
                    }
                    Ok(_) => CreateResult::Conflict,
                    Err(_) => CreateResult::Unavailable,
                }
            }
            Err(_) => CreateResult::Unavailable,
        };
        if matches!(result, CreateResult::Created(_) | CreateResult::Replayed(_))
            && tx.commit().await.is_err()
        {
            return CreateResult::Unavailable;
        }
        result
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
        let rows = sqlx::query_as::<_, (i64, i64, String)>(
            "SELECT c.id,c.author_user_id,c.body
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
                .map(|(id, author_user_id, body)| PublicComment {
                    id,
                    author_user_id,
                    body,
                })
                .collect(),
        ))
    }
}
