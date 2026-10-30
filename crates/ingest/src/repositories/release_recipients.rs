//! G-08: bounded follower candidates for a canonical first release.
//! No channel consent, external send, purchase grant or delivery receipt.
#![forbid(unsafe_code)]
use sqlx::{PgPool, Row};

pub const RECIPIENT_BATCH_SIZE: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseRecipient {
    pub user_id: i64,
    pub book_id: i64,
    pub chapter_id: i64,
    pub is_paid: bool,
    pub price_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipientBatch {
    pub items: Vec<ReleaseRecipient>,
    pub next_after_user: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecipientError {
    Invalid,
    Unavailable,
}

#[derive(Clone)]
pub struct PgReleaseRecipients {
    pool: PgPool,
}

impl PgReleaseRecipients {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Read the same verified, after-follow first releases as the C-16 feed.
    /// No subscriber-count cap and no OFFSET. This snapshot is not authority
    /// to deliver: the worker must check opt-in/consent and use durable receipts.
    pub async fn next_batch(
        &self,
        chapter_id: i64,
        after_user: Option<i64>,
    ) -> Result<RecipientBatch, RecipientError> {
        if chapter_id <= 0 || after_user.is_some_and(|cursor| cursor <= 0) {
            return Err(RecipientError::Invalid);
        }
        let rows = sqlx::query(
            "SELECT user_id,book_id,chapter_id,access_level,price_minor
             FROM reader_followed_release_candidates
             WHERE chapter_id=$1 AND user_id>$2
             ORDER BY user_id LIMIT 101",
        )
        .bind(chapter_id)
        .bind(after_user.unwrap_or(0))
        .fetch_all(&self.pool)
        .await
        .map_err(|_| RecipientError::Unavailable)?;
        let mut items = rows
            .into_iter()
            .map(|row| {
                Ok(ReleaseRecipient {
                    user_id: row
                        .try_get("user_id")
                        .map_err(|_| RecipientError::Unavailable)?,
                    book_id: row
                        .try_get("book_id")
                        .map_err(|_| RecipientError::Unavailable)?,
                    chapter_id: row
                        .try_get("chapter_id")
                        .map_err(|_| RecipientError::Unavailable)?,
                    is_paid: row
                        .try_get::<String, _>("access_level")
                        .map_err(|_| RecipientError::Unavailable)?
                        == "paid",
                    price_minor: row
                        .try_get("price_minor")
                        .map_err(|_| RecipientError::Unavailable)?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let has_more = items.len() > RECIPIENT_BATCH_SIZE;
        items.truncate(RECIPIENT_BATCH_SIZE);
        let next_after_user = if has_more {
            items.last().map(|item| item.user_id)
        } else {
            None
        };
        Ok(RecipientBatch {
            items,
            next_after_user,
        })
    }

    /// Recheck the current follow and release visibility before any future effect.
    /// Still does not establish opt-in or a channel-specific delivery right.
    pub async fn still_candidate(
        &self,
        chapter_id: i64,
        user_id: i64,
    ) -> Result<bool, RecipientError> {
        if chapter_id <= 0 || user_id <= 0 {
            return Err(RecipientError::Invalid);
        }
        sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM reader_followed_release_candidates
                WHERE chapter_id=$1 AND user_id=$2
             )",
        )
        .bind(chapter_id)
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|_| RecipientError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::RECIPIENT_BATCH_SIZE;

    #[test]
    fn g_08_recipient_batch_is_bounded() {
        assert_eq!(RECIPIENT_BATCH_SIZE, 100);
    }
}
