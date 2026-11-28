//! AUTH-READ-02c3b1: one bounded, resumable catalog sweep step.
//! No body reads, S3 requests, consumer dispatch or runtime startup.
use super::{PgGuestPrefix, MAX_GUEST_PREFIX_SWEEP_PAGE};
use crate::services::authoring::AuthorError;

pub const MAX_GUEST_PREFIX_BOOK_PAGE: i64 = 32;

/// A caller can checkpoint this metadata-only cursor between successful steps.
/// None in the result ends the current pass; a new pass starts at Books { 0 }.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuestPrefixSweepCursor {
    Books {
        after_book_id: i64,
    },
    Chapters {
        book_id: i64,
        policy_revision: i64,
        after_chapter_no: i32,
    },
}

#[derive(Debug, Eq, PartialEq)]
pub struct GuestPrefixSweepStep {
    pub next: Option<GuestPrefixSweepCursor>,
    pub enqueued: u64,
    pub stale_book: bool,
}

impl PgGuestPrefix {
    pub async fn sweep_step(
        &self,
        cursor: GuestPrefixSweepCursor,
    ) -> Result<GuestPrefixSweepStep, AuthorError> {
        use GuestPrefixSweepCursor::{Books, Chapters};
        match cursor {
            Books { after_book_id } => {
                if after_book_id < 0 {
                    return Err(AuthorError::Invalid);
                }
                // Bound the examined IDs before eligibility filtering. The
                // primary key supplies the ordering even for all-open books.
                let rows: Vec<(i64, Option<i64>)> = sqlx::query_as(
                    "WITH page AS MATERIALIZED (
                         SELECT id,guest_policy_revision,visibility,visibility_status,
                                access_level,admin_hold,is_deleted,lifecycle_status
                         FROM books WHERE id>$1 ORDER BY id LIMIT $2
                     )
                     SELECT id,CASE WHEN guest_policy_revision>0
                                      AND visibility='public' AND visibility_status='active'
                                      AND access_level='basic' AND NOT admin_hold
                                      AND NOT is_deleted
                                      AND lifecycle_status NOT IN
                                          ('pending_deletion','ordinary_deleted')
                                     THEN guest_policy_revision ELSE NULL END
                     FROM page ORDER BY id",
                )
                .bind(after_book_id)
                .bind(MAX_GUEST_PREFIX_BOOK_PAGE)
                .fetch_all(&self.pool)
                .await
                .map_err(|_| AuthorError::Unavailable)?;
                // Never skip later eligible IDs from the same page. Once
                // the selected book finishes, resume immediately after it.
                let candidate = rows.iter().find_map(|&(id, revision)| {
                    revision.map(|policy_revision| Chapters {
                        book_id: id,
                        policy_revision,
                        after_chapter_no: 0,
                    })
                });
                let next = if candidate.is_some() {
                    candidate
                } else if rows.len() == MAX_GUEST_PREFIX_BOOK_PAGE as usize {
                    rows.last().map(|row| Books {
                        after_book_id: row.0,
                    })
                } else {
                    None
                };
                Ok(GuestPrefixSweepStep {
                    next,
                    enqueued: 0,
                    stale_book: false,
                })
            }
            Chapters {
                book_id,
                policy_revision,
                after_chapter_no,
            } => {
                let result = self
                    .enqueue_missing_page(
                        book_id,
                        policy_revision,
                        after_chapter_no,
                        MAX_GUEST_PREFIX_SWEEP_PAGE,
                    )
                    .await;
                let (next, enqueued, stale_book) = match result {
                    Ok(page) => {
                        let next = match page.next_after_chapter_no {
                            Some(after_chapter_no) => Chapters {
                                book_id,
                                policy_revision,
                                after_chapter_no,
                            },
                            None => Books {
                                after_book_id: book_id,
                            },
                        };
                        (next, page.enqueued, false)
                    }
                    // Revision or eligibility changed after discovery. A new
                    // pass rediscovers it; do not spin on an obsolete cursor.
                    Err(AuthorError::Conflict) => (
                        Books {
                            after_book_id: book_id,
                        },
                        0,
                        true,
                    ),
                    Err(error) => return Err(error),
                };
                Ok(GuestPrefixSweepStep {
                    next: Some(next),
                    enqueued,
                    stale_book,
                })
            }
        }
    }
}
