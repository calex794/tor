//! E-12: version-pinned free-text corrections and author-only queue.
//! A paid version is intentionally denied until exact S3 purchase grants can
//! be verified server-side. Never trust a quote or actor supplied in HTTP.
use sqlx::{PgPool, Row};
use tor_contracts::reader_manifest::reader_blocks;

#[derive(Clone)]
pub struct PgTextIssues {
    pool: PgPool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IssueError {
    Invalid,
    NotVisible,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreatedIssue {
    Added(i64),
    Replayed(i64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusMutation {
    Changed,
    Replayed,
    NotFound,
}

#[derive(Debug, Clone)]
pub struct AuthorIssue {
    pub id: i64,
    pub chapter_id: i64,
    pub version_id: i64,
    pub block_index: i32,
    pub issue_type: String,
    pub quote: String,
    pub suggested_text: String,
    pub status: String,
    pub resolution_note: Option<String>,
}

pub struct AuthorIssuePage {
    pub book_title: String,
    pub issues: Vec<AuthorIssue>,
    pub next: Option<i64>,
}

const ISSUE_TYPES: [&str; 6] = [
    "typo",
    "formatting",
    "broken_link",
    "factual",
    "continuity",
    "other",
];
const STATUSES: [&str; 4] = ["new", "in_review", "fixed", "rejected"];

fn bounded(text: &str, min: usize, max: usize) -> bool {
    let count = text.chars().count();
    count >= min && count <= max && !text.contains('\0') && text.trim() == text
}

pub struct NewTextIssue<'a> {
    pub actor: i64,
    pub chapter_id: i64,
    pub version_id: i64,
    pub block_index: i32,
    pub selection_start: i32,
    pub selection_end: i32,
    pub quote: &'a str,
    pub suggestion: &'a str,
    pub issue_type: &'a str,
}

impl PgTextIssues {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The published, free and currently accessible ChapterVersion is locked
    /// until the insert commits. A concurrent publication cannot retarget it.
    pub async fn create_free(&self, new_issue: NewTextIssue<'_>) -> Result<CreatedIssue, IssueError> {
        let NewTextIssue {
            actor, chapter_id, version_id, block_index, selection_start,
            selection_end, quote, suggestion, issue_type,
        } = new_issue;
        if actor <= 0
            || chapter_id <= 0
            || version_id <= 0
            || !(1..=200).contains(&block_index)
            || selection_start < 0
            || selection_end <= selection_start
            || selection_end > 100_000
            || !bounded(quote, 1, 500)
            || !bounded(suggestion, 1, 1000)
            || !ISSUE_TYPES.contains(&issue_type)
        {
            return Err(IssueError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(|_| IssueError::Unavailable)?;
        let selected = sqlx::query_as::<_, (i64, String)>(
            "SELECT c.book_id,v.body_markdown
             FROM chapters c JOIN books b ON b.id=c.book_id
             JOIN chapter_publications p ON p.chapter_id=c.id
             JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=p.version_id
             WHERE c.id=$1 AND p.version_id=$2 AND b.visibility='public'
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND c.visibility='public' AND c.access_level='free'
               AND NOT c.admin_hold AND NOT c.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
             FOR SHARE OF b,c,p,v",
        )
        .bind(chapter_id)
        .bind(version_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| IssueError::Unavailable)?
        .ok_or(IssueError::NotVisible)?;
        let (book_id, source) = selected;
        let blocks = reader_blocks(&source);
        let Some(block) = usize::try_from(block_index - 1).ok().and_then(|i| blocks.get(i))
        else {
            return Err(IssueError::Invalid);
        };
        // The browser proposes UTF-16 offsets and a quote. Server checks both
        // against the exact immutable edition, including repeated text and emoji.
        let units = block.encode_utf16().collect::<Vec<_>>();
        let start = usize::try_from(selection_start).map_err(|_| IssueError::Invalid)?;
        let end = usize::try_from(selection_end).map_err(|_| IssueError::Invalid)?;
        let actual = units.get(start..end).ok_or(IssueError::Invalid)?;
        let actual = String::from_utf16(actual).map_err(|_| IssueError::Invalid)?;
        if actual != quote {
            return Err(IssueError::Invalid);
        }
        let hash_input = format!("{issue_type}\n{quote}\n{suggestion}");
        let created = sqlx::query_scalar::<_, i64>(
            "INSERT INTO text_issues
             (book_id,chapter_id,version_id,reporter_user_id,block_index,
              selection_start,selection_end,issue_type,quote,suggested_text,request_hash)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,sha256(convert_to($11,'UTF8')))
             ON CONFLICT (reporter_user_id,chapter_id,version_id,block_index,
                          selection_start,selection_end,request_hash) DO NOTHING
             RETURNING id",
        )
        .bind(book_id)
        .bind(chapter_id)
        .bind(version_id)
        .bind(actor)
        .bind(block_index)
        .bind(selection_start)
        .bind(selection_end)
        .bind(issue_type)
        .bind(quote)
        .bind(suggestion)
        .bind(&hash_input)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| IssueError::Unavailable)?;
        let result = match created {
            Some(id) => CreatedIssue::Added(id),
            None => {
                let id = sqlx::query_scalar::<_, i64>(
                    "SELECT id FROM text_issues
                     WHERE reporter_user_id=$1 AND chapter_id=$2 AND version_id=$3
                       AND block_index=$4 AND selection_start=$5 AND selection_end=$6
                       AND request_hash=sha256(convert_to($7,'UTF8'))",
                )
                .bind(actor)
                .bind(chapter_id)
                .bind(version_id)
                .bind(block_index)
                .bind(selection_start)
                .bind(selection_end)
                .bind(&hash_input)
                .fetch_one(&mut *tx)
                .await
                .map_err(|_| IssueError::Unavailable)?;
                CreatedIssue::Replayed(id)
            }
        };
        tx.commit().await.map_err(|_| IssueError::Unavailable)?;
        Ok(result)
    }

    /// Only active book-team members. Lists stay bounded and use an id keyset.
    /// Quotes are never included in public search, feeds or notifications.
    pub async fn author_page(
        &self,
        actor: i64,
        book_id: i64,
        status: Option<&str>,
        after: Option<i64>,
    ) -> Result<Option<AuthorIssuePage>, IssueError> {
        if actor <= 0 || book_id <= 0 || after.is_some_and(|id| id <= 0)
            || status.is_some_and(|s| !STATUSES.contains(&s))
        {
            return Err(IssueError::Invalid);
        }
        let title = sqlx::query_scalar::<_, String>(
            "SELECT b.title FROM books b
             JOIN book_members m ON m.book_id=b.id
             WHERE b.id=$1 AND m.user_id=$2 AND m.is_active
               AND m.role IN ('owner','translator','moderator')",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| IssueError::Unavailable)?;
        let Some(title) = title else {
            return Ok(None);
        };
        let rows = sqlx::query(
            "SELECT i.id,i.chapter_id,i.version_id,i.block_index,i.issue_type,
                    i.quote,i.suggested_text,i.status,i.resolution_note
             FROM text_issues i
             WHERE i.book_id=$1
               AND EXISTS (SELECT 1 FROM book_members m
                           WHERE m.book_id=i.book_id AND m.user_id=$4
                             AND m.is_active AND m.role IN ('owner','translator','moderator'))
               AND ($2::text IS NULL OR i.status=$2)
               AND ($3::bigint IS NULL OR i.id<$3)
             ORDER BY i.id DESC LIMIT 26",
        )
        .bind(book_id)
        .bind(status)
        .bind(after)
        .bind(actor)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| IssueError::Unavailable)?;
        let has_next = rows.len() > 25;
        let mut issues = Vec::new();
        for row in rows.into_iter().take(25) {
            issues.push(AuthorIssue {
                id: row.try_get("id").map_err(|_| IssueError::Unavailable)?,
                chapter_id: row.try_get("chapter_id").map_err(|_| IssueError::Unavailable)?,
                version_id: row.try_get("version_id").map_err(|_| IssueError::Unavailable)?,
                block_index: row.try_get("block_index").map_err(|_| IssueError::Unavailable)?,
                issue_type: row.try_get("issue_type").map_err(|_| IssueError::Unavailable)?,
                quote: row.try_get("quote").map_err(|_| IssueError::Unavailable)?,
                suggested_text: row.try_get("suggested_text").map_err(|_| IssueError::Unavailable)?,
                status: row.try_get("status").map_err(|_| IssueError::Unavailable)?,
                resolution_note: row.try_get("resolution_note").map_err(|_| IssueError::Unavailable)?,
            });
        }
        let next = if has_next {
            issues.last().map(|issue| issue.id)
        } else {
            None
        };
        Ok(Some(AuthorIssuePage {
            book_title: title,
            issues,
            next,
        }))
    }

    /// Status is a workflow decision, not a chapter mutation. The exact
    /// author check and issue lock share one transaction for safe retries.
    pub async fn update_status(
        &self,
        actor: i64,
        book_id: i64,
        issue_id: i64,
        status: &str,
        note: Option<&str>,
    ) -> Result<StatusMutation, IssueError> {
        if actor <= 0 || book_id <= 0 || issue_id <= 0
            || !STATUSES.contains(&status)
            || note.is_some_and(|value| !bounded(value, 1, 1000))
        {
            return Err(IssueError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(|_| IssueError::Unavailable)?;
        let current = sqlx::query_scalar::<_, String>(
            "SELECT i.status FROM text_issues i
             JOIN book_members m ON m.book_id=i.book_id
             WHERE i.id=$1 AND i.book_id=$2 AND m.user_id=$3
               AND m.is_active AND m.role IN ('owner','translator','moderator')
             FOR UPDATE OF i,m",
        )
        .bind(issue_id)
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| IssueError::Unavailable)?;
        let Some(current) = current else {
            return Ok(StatusMutation::NotFound);
        };
        if current == status {
            return Ok(StatusMutation::Replayed);
        }
        let terminal = matches!(status, "fixed" | "rejected");
        sqlx::query(
            "UPDATE text_issues SET status=$2,
               resolution_note=CASE WHEN $3 THEN $4 ELSE NULL END,
               resolved_at=CASE WHEN $3 THEN now() ELSE NULL END,
               resolved_by_user_id=CASE WHEN $3 THEN $5 ELSE NULL END,
               updated_at=now() WHERE id=$1",
        )
        .bind(issue_id)
        .bind(status)
        .bind(terminal)
        .bind(note)
        .bind(actor)
        .execute(&mut *tx)
        .await
        .map_err(|_| IssueError::Unavailable)?;
        sqlx::query(
            "INSERT INTO text_issue_status_events
             (issue_id,actor_user_id,old_status,new_status,resolution_note)
             VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(issue_id)
        .bind(actor)
        .bind(&current)
        .bind(status)
        .bind(note)
        .execute(&mut *tx)
        .await
        .map_err(|_| IssueError::Unavailable)?;
        tx.commit().await.map_err(|_| IssueError::Unavailable)?;
        Ok(StatusMutation::Changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn e_12_validates_inputs_and_known_categories() {
        assert!(bounded("Текст", 1, 500));
        assert!(!bounded("", 1, 500));
        assert!(!bounded("bad\0text", 1, 500));
        assert!(!bounded(" leading", 1, 500));
        assert!(!bounded(&"x".repeat(501), 1, 500));
        assert!(ISSUE_TYPES.contains(&"continuity"));
        assert!(STATUSES.contains(&"in_review"));
        assert!(!STATUSES.contains(&"unknown"));
    }
}
