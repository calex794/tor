//! Bounded D-06 swap options. Read-only metadata, never settlement authority.
use super::refund_preflight::{DirectRefundGate, PgDirectRefundPreflight};
use sqlx::{PgPool, Row};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SwapOption {
    pub source_id: i64,
    pub target_id: i64,
    pub source_book: String,
    pub source_chapter: String,
    pub target_book: String,
    pub target_chapter: String,
    pub delta_minor: i64,
}

pub async fn swap_options(pool: &PgPool, actor: i64) -> Result<Vec<SwapOption>, sqlx::Error> {
    if actor <= 0 {
        return Ok(Vec::new());
    }
    let rows=sqlx::query(
        "SELECT src.id AS source_id,target.id AS target_id,
                source_book.title AS source_book,source_ch.title AS source_chapter,
                target_book.title AS target_book,target.title AS target_chapter
         FROM chapter_purchase_facts src
         JOIN chapters source_ch ON source_ch.id=src.chapter_id
         JOIN books source_book ON source_book.id=source_ch.book_id
         JOIN books target_book ON target_book.owner_user_id=source_book.owner_user_id
             AND target_book.id<>source_book.id
         JOIN chapters target ON target.book_id=target_book.id
         JOIN chapter_publications pub ON pub.chapter_id=target.id
         WHERE src.user_id=$1 AND src.first_grant_tracking_version='c06-first-issue-v1'
           AND src.refund_policy_version IS NOT NULL AND src.refund_deadline_at>now()
           AND NOT EXISTS (SELECT 1 FROM chapter_purchase_revocations rev WHERE rev.purchase_id=src.id)
           AND NOT EXISTS (SELECT 1 FROM chapter_purchase_first_exposures e WHERE e.purchase_id=src.id)
           AND target.access_level='paid' AND target.visibility='public'
           AND target.price_amount>0 AND NOT target.admin_hold AND NOT target.is_deleted
           AND (target.reader_released_at IS NULL OR target.reader_released_at<=now())
           AND target_book.visibility='public' AND NOT target_book.admin_hold
           AND NOT target_book.is_deleted
           AND source_book.owner_user_id IS NOT NULL AND source_book.owner_user_id<>$1
           AND NOT EXISTS (SELECT 1 FROM library_purchased_chapters l
                           WHERE l.user_id=$1 AND l.chapter_id=target.id)
         ORDER BY src.id DESC,target.id DESC LIMIT 60",
    ).bind(actor).fetch_all(pool).await?;
    let preflight = PgDirectRefundPreflight::new(pool.clone());
    let mut options = Vec::new();
    for row in rows {
        let source_id: i64 = row.try_get("source_id")?;
        let target_id: i64 = row.try_get("target_id")?;
        let quote = match preflight.swap_quote(actor, source_id, target_id).await {
            Ok(q) if q.gate == DirectRefundGate::FirstGrantNotSeenInTrackedPurchase => q,
            _ => continue,
        };
        options.push(SwapOption {
            source_id,
            target_id,
            source_book: row.try_get("source_book")?,
            source_chapter: row.try_get("source_chapter")?,
            target_book: row.try_get("target_book")?,
            target_chapter: row.try_get("target_chapter")?,
            delta_minor: quote.delta_minor,
        });
    }
    Ok(options)
}
