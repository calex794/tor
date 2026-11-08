//! S011 paid chapters: canonical exact-edition comments, scoped to a live D
//! purchase and A session. Does not return sealed text, quotes or S3 grants.
#![forbid(unsafe_code)]
use sqlx::PgPool;
use tor_contracts::reader_manifest::reader_blocks;

use super::public_chapter_comments::{
    ChapterComment, ChapterCommentAnchor, ChapterCommentPage, CreateResult, ListError, NewAnchor,
    NewChapterComment,
};

#[derive(Clone)]
pub struct PgPaidChapterComments {
    pool: PgPool,
}

fn valid_comment(actor: i64, purchase: i64, c: &NewChapterComment<'_>) -> bool {
    actor > 0
        && purchase > 0
        && c.chapter_id > 0
        && c.version_id > 0
        && c.parent_id.is_none_or(|id| id > 0)
        && !c.body.is_empty()
        && c.body == c.body.trim()
        && c.body.chars().count() <= 2000
        && !c.body.contains('\0')
        && c.command_key.len() == 43
        && c.command_key
            .bytes()
            .all(|x| x.is_ascii_alphanumeric() || x == b'_' || x == b'-')
        && c.payload_hash.len() == 32
}

fn exact_anchor(source: &str, a: NewAnchor<'_>) -> bool {
    if a.block_index < 1
        || a.start_utf16 < 0
        || a.end_utf16 <= a.start_utf16
        || a.end_utf16 > 100_000
        || a.quote.is_empty()
        || a.quote.chars().count() > 500
    {
        return false;
    }
    let Some(block) = usize::try_from(a.block_index - 1)
        .ok()
        .and_then(|idx| reader_blocks(source).get(idx).cloned())
    else {
        return false;
    };
    let units: Vec<_> = block.encode_utf16().collect();
    usize::try_from(a.start_utf16)
        .ok()
        .zip(usize::try_from(a.end_utf16).ok())
        .and_then(|(start, end)| units.get(start..end))
        .and_then(|slice| String::from_utf16(slice).ok())
        .is_some_and(|text| text == a.quote)
}

impl PgPaidChapterComments {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Same transaction locks B chapter + D purchase before the write. A
    /// browser-selected version or another buyer's purchase never authorizes it.
    /// D-06 forbids tor_app UPDATE on the library pointer: that immutable
    /// pointer is read/verified, never row-locked by this query.
    pub async fn create(
        &self,
        actor: i64,
        session_digest: &[u8; 32],
        purchase: i64,
        c: NewChapterComment<'_>,
    ) -> CreateResult {
        if !valid_comment(actor, purchase, &c) {
            return CreateResult::Invalid;
        }
        let mut tx = match self.pool.begin().await {
            Ok(tx) => tx,
            Err(_) => return CreateResult::Unavailable,
        };
        let source = sqlx::query_scalar::<_, String>(
            "SELECT v.body_markdown
             FROM chapter_purchase_facts p
             JOIN library_purchased_chapters l ON l.purchase_id=p.id
               AND l.user_id=p.user_id AND l.chapter_id=p.chapter_id
               AND l.active_version_id=p.version_id
             JOIN chapters ch ON ch.id=p.chapter_id AND ch.access_level='paid'
             JOIN books b ON b.id=ch.book_id
             JOIN chapter_versions v ON v.id=p.version_id AND v.chapter_id=p.chapter_id
             JOIN users u ON u.id=p.user_id AND u.account_state='active'
             JOIN sessions s ON s.user_id=u.id AND s.token_hash=$5
               AND s.revoked_at IS NULL AND s.expires_at>now()
             WHERE p.id=$1 AND p.user_id=$2 AND p.chapter_id=$3 AND p.version_id=$4
               AND tor_purchase_version_readable($2,p.id,p.chapter_id,p.version_id)
             FOR SHARE OF p,ch,b,v,u,s",
        )
        .bind(purchase)
        .bind(actor)
        .bind(c.chapter_id)
        .bind(c.version_id)
        .bind(session_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await;
        let source = match source {
            Ok(Some(s)) => s,
            Ok(None) => return CreateResult::NotFound,
            Err(_) => return CreateResult::Unavailable,
        };
        if c.anchor.is_some_and(|a| !exact_anchor(&source, a)) {
            return CreateResult::Invalid;
        }
        if let Some(parent_id) = c.parent_id {
            let found = sqlx::query_scalar::<_, i64>(
                "SELECT id FROM discussion_comments
                 WHERE id=$1 AND chapter_id=$2 AND version_id=$3
                   AND parent_id IS NULL AND visibility='visible' FOR SHARE",
            )
            .bind(parent_id)
            .bind(c.chapter_id)
            .bind(c.version_id)
            .fetch_optional(&mut *tx)
            .await;
            match found {
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
        .bind(actor)
        .bind(c.chapter_id)
        .bind(c.version_id)
        .bind(c.body)
        .bind(c.command_key)
        .bind(c.payload_hash)
        .bind(c.parent_id)
        .fetch_optional(&mut *tx)
        .await;
        let result = match inserted {
            Ok(Some(id)) => {
                if let Some(a) = c.anchor {
                    if sqlx::query(
                        "INSERT INTO discussion_anchors
                         (comment_id,chapter_id,version_id,block_key,start_utf16,end_utf16)
                         VALUES($1,$2,$3,$4,$5,$6)",
                    )
                    .bind(id)
                    .bind(c.chapter_id)
                    .bind(c.version_id)
                    .bind(format!("b:{}", a.block_index))
                    .bind(a.start_utf16)
                    .bind(a.end_utf16)
                    .execute(&mut *tx)
                    .await
                    .is_err()
                    {
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
                .bind(actor)
                .bind(c.command_key)
                .fetch_optional(&mut *tx)
                .await;
                match original {
                    Ok(Some((id, chapter, version, hash, parent, body)))
                        if chapter == Some(c.chapter_id)
                            && version == Some(c.version_id)
                            && hash == c.payload_hash
                            && parent == c.parent_id
                            && body == c.body =>
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

    /// One SQL snapshot checks live session and D rights while returning any
    /// private comment bodies. No authorization from a historical purchase id.
    pub async fn list(
        &self,
        actor: i64,
        session_digest: &[u8; 32],
        edition: (i64, i64, i64),
        after: Option<i64>,
        limit: u16,
    ) -> Result<Option<ChapterCommentPage>, ListError> {
        let (purchase, chapter, version) = edition;
        if actor <= 0
            || purchase <= 0
            || chapter <= 0
            || version <= 0
            || limit == 0
            || limit > 100
            || after.is_some_and(|id| id <= 0)
        {
            return Err(ListError::Invalid);
        }
        let rows = sqlx::query_as::<
            _,
            (
                bool,
                Option<i64>,
                Option<i64>,
                Option<String>,
                Option<String>,
                Option<i32>,
                Option<i32>,
            ),
        >(
            "WITH allowed AS MATERIALIZED (
               SELECT p.id
               FROM chapter_purchase_facts p
               JOIN library_purchased_chapters l ON l.purchase_id=p.id
                 AND l.user_id=p.user_id AND l.chapter_id=p.chapter_id
                 AND l.active_version_id=p.version_id
               JOIN chapters ch ON ch.id=p.chapter_id AND ch.access_level='paid'
               JOIN users u ON u.id=p.user_id AND u.account_state='active'
               JOIN sessions s ON s.user_id=u.id AND s.token_hash=$5
                 AND s.revoked_at IS NULL AND s.expires_at>now()
               WHERE p.id=$1 AND p.user_id=$2 AND p.chapter_id=$3 AND p.version_id=$4
                 AND tor_purchase_version_readable($2,p.id,p.chapter_id,p.version_id)
             )
             SELECT EXISTS(SELECT 1 FROM allowed),
                    hit.id,hit.parent_id,hit.body,hit.block_key,hit.start_utf16,hit.end_utf16
             FROM (SELECT 1) singleton
             LEFT JOIN LATERAL (
               SELECT c.id,c.parent_id,c.body,a.block_key,a.start_utf16,a.end_utf16
               FROM discussion_comments c
               JOIN allowed ON TRUE
               LEFT JOIN discussion_anchors a ON a.comment_id=c.id
               WHERE c.chapter_id=$3 AND c.version_id=$4 AND c.visibility='visible'
                 AND ($6::bigint IS NULL OR c.id<$6)
                 AND (c.parent_id IS NULL OR EXISTS (
                   SELECT 1 FROM discussion_comments parent
                   WHERE parent.id=c.parent_id AND parent.chapter_id=$3
                     AND parent.version_id=$4 AND parent.visibility='visible'))
               ORDER BY c.id DESC LIMIT $7
             ) hit ON TRUE",
        )
        .bind(purchase)
        .bind(actor)
        .bind(chapter)
        .bind(version)
        .bind(session_digest.as_slice())
        .bind(after)
        .bind(i64::from(limit) + 1)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| ListError::Unavailable)?;
        if !rows.first().is_some_and(|row| row.0) {
            return Ok(None);
        }
        let has_next = rows.iter().filter(|row| row.1.is_some()).count() > usize::from(limit);
        let items: Vec<_> = rows
            .into_iter()
            .filter_map(|(_, id, parent, body, block, start, end)| {
                Some(ChapterComment {
                    id: id?,
                    parent_id: parent,
                    body: body?,
                    anchor: block
                        .as_deref()
                        .and_then(|key| key.strip_prefix("b:"))
                        .and_then(|key| key.parse::<i32>().ok())
                        .zip(start)
                        .zip(end)
                        .map(
                            |((block_index, start_utf16), end_utf16)| ChapterCommentAnchor {
                                block_index,
                                start_utf16,
                                end_utf16,
                            },
                        ),
                })
            })
            .take(usize::from(limit))
            .collect();
        let next_cursor = has_next.then(|| items.last().map(|i| i.id)).flatten();
        Ok(Some(ChapterCommentPage { items, next_cursor }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s011_paid_comments_validate_utf16_and_refuse_forged_quote() {
        let source = "А😀Б\n\nВторая часть";
        let anchor = NewAnchor {
            block_index: 1,
            start_utf16: 1,
            end_utf16: 3,
            quote: "😀",
        };
        assert!(exact_anchor(source, anchor));
        assert!(!exact_anchor(
            source,
            NewAnchor {
                quote: "А",
                ..anchor
            }
        ));
        assert!(!exact_anchor(
            source,
            NewAnchor {
                end_utf16: 2,
                ..anchor
            }
        ));
    }
}
