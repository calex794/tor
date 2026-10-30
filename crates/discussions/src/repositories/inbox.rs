//! E-03: a private projection of existing canonical comments, not another message store.
use super::public_book_comments::{ListCommentsError, ReceivedReply};
use sqlx::PgPool;

#[derive(Debug, Eq, PartialEq)]
pub struct InboxEntry {
    pub reply: ReceivedReply,
    pub created_at: String,
    pub is_mention: bool,
}

#[derive(Clone)]
pub struct PgInbox {
    pool: PgPool,
}

impl PgInbox {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// A recipient is derived only from the verified actor and the canonical parent.
    /// Every read rechecks visibility of both comments and the book; no body is read.
    pub async fn received(
        &self,
        actor_id: i64,
        after_id: Option<i64>,
        read: Option<bool>,
        limit: u16,
    ) -> Result<Vec<InboxEntry>, ListCommentsError> {
        if actor_id <= 0 || limit == 0 || limit > 100 || after_id.is_some_and(|n| n <= 0) {
            return Err(ListCommentsError::Invalid);
        }
        let rows = sqlx::query_as::<_, (i64, i64, String, bool, String, bool)>(
            r#"SELECT reply.id, b.id, b.title, (seen.reply_comment_id IS NOT NULL),
                      to_char(reply.created_at AT TIME ZONE 'UTC',
                              'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'),
                      (mention.recipient_user_id IS NOT NULL)
               FROM discussion_comments reply
               LEFT JOIN discussion_comments parent ON parent.id=reply.parent_id
               JOIN books b ON b.id=reply.book_id
               LEFT JOIN discussion_reply_reads seen
                 ON seen.recipient_user_id=$1 AND seen.reply_comment_id=reply.id
               LEFT JOIN discussion_comment_mentions mention
                 ON mention.comment_id=reply.id AND mention.recipient_user_id=$1
               WHERE (parent.author_user_id=$1 OR mention.recipient_user_id IS NOT NULL)
                 AND reply.author_user_id<>$1
                 AND (parent.id IS NULL OR parent.book_id=reply.book_id)
                 AND (parent.id IS NULL OR (parent.chapter_id IS NULL
                       AND parent.version_id IS NULL AND parent.visibility='visible'))
                 AND reply.chapter_id IS NULL AND reply.version_id IS NULL
                 AND reply.visibility='visible'
                 AND b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
                 AND ($2::BIGINT IS NULL OR reply.id < $2)
                 AND ($3::BOOLEAN IS NULL OR (seen.reply_comment_id IS NOT NULL) = $3)
               ORDER BY reply.id DESC LIMIT $4"#,
        )
        .bind(actor_id)
        .bind(after_id)
        .bind(read)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(|_| ListCommentsError::Unavailable)?;
        Ok(rows
            .into_iter()
            .map(
                |(comment_id, book_id, book_title, is_read, created_at, is_mention)| InboxEntry {
                    reply: ReceivedReply {
                        comment_id,
                        book_id,
                        book_title,
                        is_read,
                    },
                    created_at,
                    is_mention,
                },
            )
            .collect())
    }
}
