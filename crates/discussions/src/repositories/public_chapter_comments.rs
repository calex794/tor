//! E-02/S011: exact-edition comments for *currently published free* chapters.
//! The canonical E-01 tables already store version-bound comments and anchors.
//! Paid/private chapters must use their own D entitlement checks, never this repo.
#![forbid(unsafe_code)]
use sqlx::PgPool;
use tor_contracts::reader_manifest::reader_blocks;

#[derive(Clone)]
pub struct PgPublicChapterComments {
    pool: PgPool,
}

#[derive(Clone, Copy, Debug)]
pub struct NewAnchor<'a> {
    pub block_index: i32,
    pub start_utf16: i32,
    pub end_utf16: i32,
    pub quote: &'a str,
}

#[derive(Clone, Copy, Debug)]
pub struct NewChapterComment<'a> {
    pub chapter_id: i64,
    pub version_id: i64,
    pub body: &'a str,
    pub command_key: &'a str,
    pub payload_hash: &'a [u8],
    pub parent_id: Option<i64>,
    pub anchor: Option<NewAnchor<'a>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreateResult {
    Created(i64),
    Replayed(i64),
    Invalid,
    Conflict,
    NotFound,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListError {
    Invalid,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChapterCommentAnchor {
    pub block_index: i32,
    pub start_utf16: i32,
    pub end_utf16: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChapterComment {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub body: String,
    pub anchor: Option<ChapterCommentAnchor>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChapterCommentPage {
    pub items: Vec<ChapterComment>,
    pub next_cursor: Option<i64>,
}

/// A source-exact UTF-16 quote; no uncontrolled fragment text is persisted in
/// discussion_anchors. This rejects bisected emoji and Markdown source gaps.
fn valid_anchor(source: &str, anchor: NewAnchor<'_>) -> bool {
    if anchor.block_index <= 0
        || anchor.start_utf16 < 0
        || anchor.end_utf16 <= anchor.start_utf16
        || anchor.end_utf16 > 100_000
        || anchor.quote.is_empty()
        || anchor.quote.chars().count() > 500
    {
        return false;
    }
    let blocks = reader_blocks(source);
    let Some(block) = usize::try_from(anchor.block_index - 1)
        .ok()
        .and_then(|index| blocks.get(index))
    else {
        return false;
    };
    let start = anchor.start_utf16 as usize;
    let end = anchor.end_utf16 as usize;
    let units = block.encode_utf16().collect::<Vec<_>>();
    units
        .get(start..end)
        .and_then(|part| String::from_utf16(part).ok())
        .is_some_and(|text| text == anchor.quote)
}

fn valid_request(actor_id: i64, comment: &NewChapterComment<'_>) -> bool {
    actor_id > 0
        && comment.chapter_id > 0
        && comment.version_id > 0
        && comment.parent_id.is_none_or(|id| id > 0)
        && !comment.body.is_empty()
        && comment.body.trim() == comment.body
        && comment.body.chars().count() <= 2000
        && !comment.body.contains('\0')
        && comment.command_key.len() == 43
        && comment
            .command_key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        && comment.payload_hash.len() == 32
}

impl PgPublicChapterComments {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Exact author + edition + command identity, transactional publication and
    /// moderation checks, no inferred paid grants or cross-edition parents.
    pub async fn create_free(&self, actor_id: i64, comment: NewChapterComment<'_>) -> CreateResult {
        if !valid_request(actor_id, &comment) {
            return CreateResult::Invalid;
        }
        let mut tx = match self.pool.begin().await {
            Ok(tx) => tx,
            Err(_) => return CreateResult::Unavailable,
        };
        let source = sqlx::query_scalar::<_, String>(
            "SELECT v.body_markdown FROM chapters c
             JOIN books b ON b.id=c.book_id
             JOIN users actor ON actor.id=$3 AND actor.account_state='active'
             JOIN chapter_publications p ON p.chapter_id=c.id
             JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=p.version_id
             WHERE c.id=$1 AND p.version_id=$2 AND b.visibility='public' AND b.visibility_status='active'
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND c.visibility='public' AND c.access_level='free'
               AND NOT c.admin_hold AND NOT c.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
               AND (b.access_level='basic'
                 OR (b.access_level='adult' AND actor.adult_enabled)
                 OR (b.access_level='pro_adult' AND EXISTS (
                   SELECT 1 FROM user_level_grants age
                   WHERE age.user_id=actor.id AND age.level='pro_adult'
                     AND age.is_active FOR SHARE OF age)))
             FOR SHARE OF b,c,p,v,actor",
        )
        .bind(comment.chapter_id)
        .bind(comment.version_id)
        .bind(actor_id)
        .fetch_optional(&mut *tx)
        .await;
        let source = match source {
            Ok(Some(source)) => source,
            Ok(None) => return CreateResult::NotFound,
            Err(_) => return CreateResult::Unavailable,
        };
        if comment.anchor.is_some_and(|a| !valid_anchor(&source, a)) {
            return CreateResult::Invalid;
        }
        if let Some(parent_id) = comment.parent_id {
            let parent = sqlx::query_scalar::<_, i64>(
                "SELECT id FROM discussion_comments
                 WHERE id=$1 AND chapter_id=$2 AND version_id=$3
                   AND parent_id IS NULL AND visibility='visible' FOR SHARE",
            )
            .bind(parent_id)
            .bind(comment.chapter_id)
            .bind(comment.version_id)
            .fetch_optional(&mut *tx)
            .await;
            match parent {
                Ok(Some(_)) => {}
                Ok(None) => return CreateResult::NotFound,
                Err(_) => return CreateResult::Unavailable,
            }
        }
        let inserted = sqlx::query_scalar::<_, i64>(
            "INSERT INTO discussion_comments
              (author_user_id,chapter_id,version_id,body,command_key,command_hash,visibility,parent_id)
             VALUES($1,$2,$3,$4,$5,$6,'visible',$7)
             ON CONFLICT (author_user_id,command_key) DO NOTHING RETURNING id",
        )
        .bind(actor_id)
        .bind(comment.chapter_id)
        .bind(comment.version_id)
        .bind(comment.body)
        .bind(comment.command_key)
        .bind(comment.payload_hash)
        .bind(comment.parent_id)
        .fetch_optional(&mut *tx)
        .await;
        let result = match inserted {
            Ok(Some(id)) => {
                if let Some(anchor) = comment.anchor {
                    let added = sqlx::query(
                        "INSERT INTO discussion_anchors
                         (comment_id,chapter_id,version_id,block_key,start_utf16,end_utf16)
                         VALUES($1,$2,$3,$4,$5,$6)",
                    )
                    .bind(id)
                    .bind(comment.chapter_id)
                    .bind(comment.version_id)
                    .bind(format!("b:{}", anchor.block_index))
                    .bind(anchor.start_utf16)
                    .bind(anchor.end_utf16)
                    .execute(&mut *tx)
                    .await;
                    if added.is_err() {
                        return CreateResult::Unavailable;
                    }
                }
                CreateResult::Created(id)
            }
            Ok(None) => {
                let original = sqlx::query_as::<
                    _,
                    (i64, Option<i64>, Option<i64>, Vec<u8>, Option<i64>, String),
                >(
                    "SELECT id,chapter_id,version_id,command_hash,parent_id,body
                     FROM discussion_comments WHERE author_user_id=$1 AND command_key=$2",
                )
                .bind(actor_id)
                .bind(comment.command_key)
                .fetch_optional(&mut *tx)
                .await;
                match original {
                    Ok(Some((id, chapter_id, version_id, hash, parent_id, body)))
                        if chapter_id == Some(comment.chapter_id)
                            && version_id == Some(comment.version_id)
                            && hash == comment.payload_hash
                            && parent_id == comment.parent_id
                            && body == comment.body =>
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

    /// Every page rechecks public free publication; a future update or
    /// revocation must never expose old-edition, paid or hidden discussions.
    pub async fn list_free(
        &self,
        chapter_id: i64,
        version_id: i64,
        after_id: Option<i64>,
        limit: u16,
    ) -> Result<Option<ChapterCommentPage>, ListError> {
        self.list_free_for_session(chapter_id, version_id, after_id, limit, None)
            .await
    }

    /// Recheck the actor's live age right in the statement returning comment
    /// bodies. The public wrapper intentionally allows basic books only.
    pub async fn list_free_for_session(
        &self,
        chapter_id: i64,
        version_id: i64,
        after_id: Option<i64>,
        limit: u16,
        session_digest: Option<&[u8; 32]>,
    ) -> Result<Option<ChapterCommentPage>, ListError> {
        if chapter_id <= 0
            || version_id <= 0
            || limit == 0
            || limit > 100
            || after_id.is_some_and(|id| id <= 0)
        {
            return Err(ListError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| ListError::Unavailable)?;
        let permitted = sqlx::query_scalar::<_, i64>(
            "SELECT v.id FROM chapters c
             JOIN books b ON b.id=c.book_id
             JOIN chapter_publications p ON p.chapter_id=c.id
             JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=p.version_id
             WHERE c.id=$1 AND p.version_id=$2 AND b.visibility='public' AND b.visibility_status='active'
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND c.visibility='public' AND c.access_level='free'
               AND NOT c.admin_hold AND NOT c.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
               AND (b.access_level='basic' OR EXISTS (
                 SELECT 1 FROM sessions sess JOIN users acct ON acct.id=sess.user_id
                 WHERE sess.token_hash=$3::bytea AND sess.revoked_at IS NULL
                   AND sess.expires_at>now() AND acct.account_state='active'
                   AND ((b.access_level='adult' AND acct.adult_enabled)
                     OR (b.access_level='pro_adult' AND EXISTS (
                       SELECT 1 FROM user_level_grants age
                       WHERE age.user_id=acct.id AND age.level='pro_adult' AND age.is_active))))
             FOR SHARE OF b,c,p,v",
        )
        .bind(chapter_id)
        .bind(version_id)
        .bind(session_digest.map(|digest| digest.as_slice()))
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| ListError::Unavailable)?;
        if permitted.is_none() {
            return Ok(None);
        }
        let rows = sqlx::query_as::<
            _,
            (
                i64,
                Option<i64>,
                String,
                Option<String>,
                Option<i32>,
                Option<i32>,
            ),
        >(
            "SELECT c.id,c.parent_id,c.body,a.block_key,a.start_utf16,a.end_utf16
             FROM discussion_comments c
             JOIN chapters ch ON ch.id=c.chapter_id
             JOIN books b ON b.id=ch.book_id
             JOIN chapter_publications p ON p.chapter_id=ch.id AND p.version_id=c.version_id
             LEFT JOIN discussion_anchors a ON a.comment_id=c.id
             WHERE c.chapter_id=$1 AND c.version_id=$2
               AND c.visibility='visible'
               AND b.visibility='public' AND b.visibility_status='active'
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND ch.visibility='public' AND ch.access_level='free'
               AND NOT ch.admin_hold AND NOT ch.is_deleted
               AND (ch.reader_released_at IS NULL OR ch.reader_released_at<=now())
               AND (b.access_level='basic' OR EXISTS (
                 SELECT 1 FROM sessions sess JOIN users acct ON acct.id=sess.user_id
                 WHERE sess.token_hash=$5::bytea AND sess.revoked_at IS NULL
                   AND sess.expires_at>now() AND acct.account_state='active'
                   AND ((b.access_level='adult' AND acct.adult_enabled)
                     OR (b.access_level='pro_adult' AND EXISTS (
                       SELECT 1 FROM user_level_grants age
                       WHERE age.user_id=acct.id AND age.level='pro_adult' AND age.is_active))))
               AND ($3::bigint IS NULL OR c.id<$3)
               AND (c.parent_id IS NULL OR EXISTS(
                 SELECT 1 FROM discussion_comments parent
                 WHERE parent.id=c.parent_id AND parent.chapter_id=$1
                   AND parent.version_id=$2 AND parent.visibility='visible'))
             ORDER BY c.id DESC LIMIT $4",
        )
        .bind(chapter_id)
        .bind(version_id)
        .bind(after_id)
        .bind(i64::from(limit) + 1)
        .bind(session_digest.map(|digest| digest.as_slice()))
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| ListError::Unavailable)?;
        let has_next = rows.len() > usize::from(limit);
        let items = rows
            .into_iter()
            .take(usize::from(limit))
            .map(|(id, parent_id, body, block_key, start_utf16, end_utf16)| {
                let anchor = block_key
                    .as_deref()
                    .and_then(|key| key.strip_prefix("b:"))
                    .and_then(|number| number.parse::<i32>().ok())
                    .zip(start_utf16)
                    .zip(end_utf16)
                    .map(
                        |((block_index, start_utf16), end_utf16)| ChapterCommentAnchor {
                            block_index,
                            start_utf16,
                            end_utf16,
                        },
                    );
                ChapterComment {
                    id,
                    parent_id,
                    body,
                    anchor,
                }
            })
            .collect::<Vec<_>>();
        let next_cursor = has_next.then(|| items.last().map(|item| item.id)).flatten();
        tx.commit().await.map_err(|_| ListError::Unavailable)?;
        Ok(Some(ChapterCommentPage { items, next_cursor }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s011_emoji_and_markdown_quotes_require_exact_source_offsets() {
        let markdown = "A😀Z\n\n**Привет** и текст";
        assert!(valid_anchor(
            markdown,
            NewAnchor {
                block_index: 1,
                start_utf16: 1,
                end_utf16: 3,
                quote: "😀",
            }
        ));
        assert!(!valid_anchor(
            markdown,
            NewAnchor {
                block_index: 1,
                start_utf16: 1,
                end_utf16: 2,
                quote: "😀",
            }
        ));
        assert!(valid_anchor(
            markdown,
            NewAnchor {
                block_index: 2,
                start_utf16: 2,
                end_utf16: 8,
                quote: "Привет",
            }
        ));
        assert!(!valid_anchor(
            markdown,
            NewAnchor {
                block_index: 2,
                start_utf16: 0,
                end_utf16: 6,
                quote: "Привет",
            }
        ));
        assert!(!valid_anchor(
            markdown,
            NewAnchor {
                block_index: 3,
                start_utf16: 0,
                end_utf16: 1,
                quote: "X",
            }
        ));
    }
}
