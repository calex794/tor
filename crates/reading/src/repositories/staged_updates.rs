//! Reader-scoped, on-site V2+ notifications after durable G-08 staging.
//! C-16 already handles first releases. No email/push, entitlement or text body.
use sqlx::{PgPool, Row};
const PAGE_SIZE: usize = 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevisionCursor {
    pub staged_micros: i64,
    pub event_id: String,
}

fn valid_uuid(id: &str) -> bool {
    id.len() == 36
        && id.as_bytes().iter().enumerate().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                *c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

impl RevisionCursor {
    pub fn parse(raw: &str) -> Option<Self> {
        if raw.len() > 64 {
            return None;
        }
        let (micros, id) = raw.split_once('.')?;
        let staged_micros = micros.parse::<i64>().ok()?;
        (staged_micros > 0 && valid_uuid(id)).then(|| Self {
            staged_micros,
            event_id: id.to_owned(),
        })
    }
    pub fn encode(&self) -> String {
        format!("{}.{}", self.staged_micros, self.event_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedRevision {
    pub book_id: i64,
    pub chapter_id: i64,
    pub book_title: String,
    pub chapter_title: String,
    pub chapter_no: i32,
    pub version_no: i32,
    pub is_paid: bool,
    pub is_read: bool,
    pub staged_micros: i64,
    pub event_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedRevisionPage {
    pub items: Vec<StagedRevision>,
    pub next_cursor: Option<RevisionCursor>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StagedRevisionError {
    Invalid,
    Unavailable,
}

#[derive(Clone)]
pub struct PgStagedRevisions {
    pool: PgPool,
}

impl PgStagedRevisions {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Only completed internal staging for this authenticated actor.
    /// Recheck current subscription, published edition and visibility at
    /// every read. Returned metadata cannot grant access to paid chapter body.
    pub async fn latest(
        &self,
        actor: i64,
        before: Option<RevisionCursor>,
    ) -> Result<StagedRevisionPage, StagedRevisionError> {
        if actor <= 0
            || before
                .as_ref()
                .is_some_and(|cursor| cursor.staged_micros <= 0 || !valid_uuid(&cursor.event_id))
        {
            return Err(StagedRevisionError::Invalid);
        }
        let rows = sqlx::query(
            r#"SELECT book_id,book_title,chapter_id,chapter_title,
                      chapter_no,version_no,is_paid,
                      (EXTRACT(EPOCH FROM staged_at)*1000000)::bigint
                        AS staged_micros,
                      event_id::text AS event_id,
                      (read_at IS NOT NULL) AS is_read
               FROM reader_staged_revision_candidates
               WHERE user_id=$1
                 AND ($2::bigint IS NULL OR
                      (staged_at,event_id)<
                      (TIMESTAMPTZ 'epoch' + $2::bigint * INTERVAL '1 microsecond',
                       $3::uuid))
               ORDER BY staged_at DESC,event_id DESC LIMIT 21"#,
        )
        .bind(actor)
        .bind(before.as_ref().map(|c| c.staged_micros))
        .bind(before.as_ref().map(|c| c.event_id.as_str()))
        .fetch_all(&self.pool)
        .await
        .map_err(|_| StagedRevisionError::Unavailable)?;

        let mut items: Vec<StagedRevision> = rows
            .into_iter()
            .map(|row| {
                Ok(StagedRevision {
                    book_id: row
                        .try_get("book_id")
                        .map_err(|_| StagedRevisionError::Unavailable)?,
                    chapter_id: row
                        .try_get("chapter_id")
                        .map_err(|_| StagedRevisionError::Unavailable)?,
                    book_title: row
                        .try_get("book_title")
                        .map_err(|_| StagedRevisionError::Unavailable)?,
                    chapter_title: row
                        .try_get("chapter_title")
                        .map_err(|_| StagedRevisionError::Unavailable)?,
                    chapter_no: row
                        .try_get("chapter_no")
                        .map_err(|_| StagedRevisionError::Unavailable)?,
                    version_no: row
                        .try_get("version_no")
                        .map_err(|_| StagedRevisionError::Unavailable)?,
                    is_paid: row
                        .try_get("is_paid")
                        .map_err(|_| StagedRevisionError::Unavailable)?,
                    is_read: row
                        .try_get("is_read")
                        .map_err(|_| StagedRevisionError::Unavailable)?,
                    staged_micros: row
                        .try_get("staged_micros")
                        .map_err(|_| StagedRevisionError::Unavailable)?,
                    event_id: row
                        .try_get("event_id")
                        .map_err(|_| StagedRevisionError::Unavailable)?,
                })
            })
            .collect::<Result<_, StagedRevisionError>>()?;
        let has_more = items.len() > PAGE_SIZE;
        items.truncate(PAGE_SIZE);
        let next_cursor = if has_more {
            items.last().map(|item| RevisionCursor {
                staged_micros: item.staged_micros,
                event_id: item.event_id.clone(),
            })
        } else {
            None
        };
        Ok(StagedRevisionPage { items, next_cursor })
    }

    /// Only the currently visible authenticated reader may explicitly mark
    /// their own event read/unread. Repeating a command is idempotent and
    /// never touches the V1 per-chapter read state or paid authorization.
    pub async fn set_read_state(
        &self,
        actor: i64,
        event_id: &str,
        read: bool,
    ) -> Result<bool, StagedRevisionError> {
        if actor <= 0 || !valid_uuid(event_id) {
            return Err(StagedRevisionError::Invalid);
        }
        let changed: bool = sqlx::query_scalar(
            "WITH eligible AS (
               SELECT 1 FROM reader_staged_revision_candidates
               WHERE user_id=$1 AND event_id=$2::uuid
             ), updated AS (
               UPDATE notification_event_recipient_intents AS intent
               SET read_at=CASE WHEN $3 THEN COALESCE(intent.read_at,now()) ELSE NULL END
               WHERE intent.user_id=$1 AND intent.event_id=$2::uuid
                 AND EXISTS(SELECT 1 FROM eligible)
               RETURNING intent.event_id
             )
             SELECT EXISTS(SELECT 1 FROM updated)",
        )
        .bind(actor)
        .bind(event_id)
        .bind(read)
        .fetch_one(&self.pool)
        .await
        .map_err(|_| StagedRevisionError::Unavailable)?;
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn revision_cursor_requires_valid_timestamp_and_uuid() {
        let valid = RevisionCursor {
            staged_micros: 1_800_000_000_123_456,
            event_id: "01234567-89ab-cdef-0123-456789abcdef".into(),
        };
        assert_eq!(RevisionCursor::parse(&valid.encode()), Some(valid));
        for invalid in [
            "",
            "1",
            "-1.01234567-89ab-cdef-0123-456789abcdef",
            "0.01234567-89ab-cdef-0123-456789abcdef",
            "1.x",
            "1.01234567-89ab-cdef-0123-456789abcdez",
            "1.0123456789abcdef0123456789abcdef",
            "1.01234567-89ab-cdef-0123-456789abcdef.extra",
        ] {
            assert!(RevisionCursor::parse(invalid).is_none());
        }
    }
}
