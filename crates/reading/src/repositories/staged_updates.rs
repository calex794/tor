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
        if actor <= 0 || before.as_ref().is_some_and(|cursor| {
            cursor.staged_micros <= 0 || !valid_uuid(&cursor.event_id)
        }) {
            return Err(StagedRevisionError::Invalid);
        }
        let rows = sqlx::query(
            r#"SELECT book.id AS book_id,book.title AS book_title,
                      chapter.id AS chapter_id,chapter.title AS chapter_title,
                      chapter.chapter_no,version.version_no,
                      (chapter.access_level='paid') AS is_paid,
                      (EXTRACT(EPOCH FROM intent.staged_at)*1000000)::bigint
                        AS staged_micros,
                      event.id::text AS event_id
               FROM notification_event_recipient_intents AS intent
               JOIN notification_event_fanout_scans AS scan
                 ON scan.event_id=intent.event_id
                AND scan.completed_at IS NOT NULL
               JOIN event_deliveries AS receipt
                 ON receipt.event_id=intent.event_id
                AND receipt.consumer='subscription-inbox-v1'
               JOIN outbox_events AS event ON event.id=intent.event_id
               JOIN chapters AS chapter ON chapter.id=event.aggregate_id
                 AND event.payload->'chapter_id'=to_jsonb(chapter.id)
               JOIN books AS book ON book.id=chapter.book_id
                 AND event.payload->'book_id'=to_jsonb(book.id)
               JOIN chapter_publications AS publication
                 ON publication.chapter_id=chapter.id
                AND event.payload->'version_id'=to_jsonb(publication.version_id)
               JOIN chapter_versions AS version ON version.id=publication.version_id
               JOIN reader_book_follows AS follows ON follows.book_id=book.id
                 AND follows.user_id=intent.user_id
               WHERE intent.user_id=$1
                 AND event.event_type='ChapterPublished'
                 AND event.schema_version=1
                 AND event.aggregate_kind='chapter'
                 AND event.payload->'first_release'='false'::jsonb
                 AND follows.created_at<=event.created_at
                 AND book.visibility='public'
                 AND NOT book.admin_hold AND NOT book.is_deleted
                 AND chapter.visibility='public'
                 AND NOT chapter.admin_hold AND NOT chapter.is_deleted
                 AND (chapter.reader_released_at IS NULL
                      OR chapter.reader_released_at<=now())
                 AND (
                   (chapter.access_level='free' AND chapter.price_amount=0)
                   OR (
                     chapter.access_level='paid' AND chapter.price_amount>0
                     AND EXISTS (
                       SELECT 1 FROM chapter_encrypted_release_events AS encrypted
                       JOIN chapter_versions AS protected
                         ON protected.id=encrypted.version_id
                        AND protected.chapter_id=encrypted.chapter_id
                       JOIN storage_objects AS object
                         ON object.id=encrypted.storage_object_id
                        AND object.id=protected.storage_object_id
                       WHERE encrypted.chapter_id=chapter.id
                         AND encrypted.version_id=publication.version_id
                         AND object.state='attached'
                         AND object.object_kind='chapter'
                     )
                   )
                 )
                 AND ($2::bigint IS NULL OR
                      (intent.staged_at,event.id)<
                      (TIMESTAMPTZ 'epoch' + $2::bigint * INTERVAL '1 microsecond',
                       $3::uuid))
               ORDER BY intent.staged_at DESC,event.id DESC LIMIT 21"#,
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
                    book_id: row.try_get("book_id").map_err(|_| StagedRevisionError::Unavailable)?,
                    chapter_id: row.try_get("chapter_id").map_err(|_| StagedRevisionError::Unavailable)?,
                    book_title: row.try_get("book_title").map_err(|_| StagedRevisionError::Unavailable)?,
                    chapter_title: row.try_get("chapter_title").map_err(|_| StagedRevisionError::Unavailable)?,
                    chapter_no: row.try_get("chapter_no").map_err(|_| StagedRevisionError::Unavailable)?,
                    version_no: row.try_get("version_no").map_err(|_| StagedRevisionError::Unavailable)?,
                    is_paid: row.try_get("is_paid").map_err(|_| StagedRevisionError::Unavailable)?,
                    staged_micros: row.try_get("staged_micros").map_err(|_| StagedRevisionError::Unavailable)?,
                    event_id: row.try_get("event_id").map_err(|_| StagedRevisionError::Unavailable)?,
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
            "", "1", "-1.01234567-89ab-cdef-0123-456789abcdef",
            "0.01234567-89ab-cdef-0123-456789abcdef",
            "1.x", "1.01234567-89ab-cdef-0123-456789abcdez",
            "1.0123456789abcdef0123456789abcdef",
            "1.01234567-89ab-cdef-0123-456789abcdef.extra",
        ] {
            assert!(RevisionCursor::parse(invalid).is_none());
        }
    }
}
