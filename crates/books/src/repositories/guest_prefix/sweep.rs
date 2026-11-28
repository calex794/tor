//! B-owned bounded discovery for the future fenced G preparation consumer.
//! Discovery does not grant reading, claim a lease or commit a projection.
use super::{lock_prepared_book, GuestPrefixTarget, PgGuestPrefix};
use crate::services::authoring::AuthorError;
use tor_contracts::guest_prefix::GUEST_PREFIX_ALGORITHM;

pub const MAX_GUEST_PREFIX_SWEEP_PAGE: i64 = 64;

#[derive(Debug, Eq, PartialEq)]
pub struct GuestPrefixPage {
    pub targets: Vec<GuestPrefixTarget>,
    /// Continue after the last inspected chapter number, not the last
    /// eligible target. None means this bounded sweep has reached its end.
    pub next_after_chapter_no: Option<i32>,
}

impl PgGuestPrefix {
    pub async fn missing_page(
        &self,
        book_id: i64,
        policy_revision: i64,
        after_chapter_no: i32,
        limit: i64,
    ) -> Result<GuestPrefixPage, AuthorError> {
        if book_id <= 0
            || policy_revision <= 0
            || after_chapter_no < 0
            || !(1..=MAX_GUEST_PREFIX_SWEEP_PAGE).contains(&limit)
        {
            return Err(AuthorError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        let cutoff = lock_prepared_book(&mut tx, book_id, policy_revision).await?;
        // Bound the *examined* rows, even if a large range is paid, held or
        // already prepared. The existing unique book_id/chapter_no index
        // supports this keyset; no OFFSET, source body or object lookup.
        let rows: Vec<(i32, i64, Option<i64>)> = sqlx::query_as(
            "WITH page AS MATERIALIZED (
                 SELECT id,chapter_no,visibility,access_level,price_amount,
                        admin_hold,is_deleted,reader_released_at
                 FROM chapters
                 WHERE book_id=$1 AND chapter_no>$2
                 ORDER BY chapter_no LIMIT $3
             )
             SELECT c.chapter_no,c.id,
                    CASE WHEN c.visibility='public' AND c.access_level='free'
                          AND c.price_amount=0 AND NOT c.admin_hold AND NOT c.is_deleted
                          AND (c.reader_released_at IS NULL
                               OR c.reader_released_at<=clock_timestamp())
                          AND g.version_id IS NULL
                         THEN p.version_id ELSE NULL END
             FROM page c
             LEFT JOIN chapter_publications p ON p.chapter_id=c.id
             LEFT JOIN chapter_version_guest_prefixes g
               ON g.chapter_id=c.id AND g.version_id=p.version_id
              AND g.algorithm_version=$4
             ORDER BY c.chapter_no",
        )
        .bind(book_id)
        .bind(cutoff.max(after_chapter_no))
        .bind(limit)
        .bind(GUEST_PREFIX_ALGORITHM)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let next_after_chapter_no = if rows.len() == limit as usize {
            rows.last().map(|row| row.0)
        } else {
            None
        };
        let targets = rows
            .into_iter()
            .filter_map(|(_, chapter_id, version_id)| {
                version_id.map(|version_id| GuestPrefixTarget {
                    book_id,
                    chapter_id,
                    version_id,
                    policy_revision,
                })
            })
            .collect();
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(GuestPrefixPage {
            targets,
            next_after_chapter_no,
        })
    }
}
