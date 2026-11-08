//! Private browser offers; no money or entitlement writes.
use super::{unavailable, PgCheckout};
use crate::domain::money::CheckoutError;
use sqlx::Row;

/// Server-verified paid offer; no plaintext, object key, grant or entitlement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedChapterOffer {
    pub book_id: i64,
    pub book_title: String,
    pub chapter_id: i64,
    pub chapter_no: i32,
    pub chapter_title: String,
    pub version_id: i64,
    pub price_minor: i64,
}

impl PgCheckout {
    /// The browser may offer a purchase only for the current published paid
    /// edition whose private encrypted S3 object was B/G verified and attached.
    /// D still rechecks the exact price/version with row locks at debit time.
    pub async fn published_chapter_offer(
        &self,
        chapter_id: i64,
    ) -> Result<Option<PublishedChapterOffer>, CheckoutError> {
        if chapter_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT b.id AS book_id, b.title AS book_title,
                    c.id AS chapter_id, c.chapter_no, c.title AS chapter_title,
                    p.version_id, c.price_amount
             FROM chapters c
             JOIN books b ON b.id=c.book_id
             JOIN chapter_publications p ON p.chapter_id=c.id
             JOIN chapter_versions v ON v.id=p.version_id AND v.chapter_id=c.id
             JOIN storage_objects o ON o.id=v.storage_object_id
                 AND o.state='attached' AND o.object_kind='chapter'
             WHERE c.id=$1 AND c.visibility='public' AND c.access_level='paid'
               AND c.price_amount>0 AND NOT c.admin_hold AND NOT c.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
               AND b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted",
        )
        .bind(chapter_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(unavailable)?;
        row.map(|row| {
            Ok(PublishedChapterOffer {
                book_id: row.try_get("book_id").map_err(unavailable)?,
                book_title: row.try_get("book_title").map_err(unavailable)?,
                chapter_id: row.try_get("chapter_id").map_err(unavailable)?,
                chapter_no: row.try_get("chapter_no").map_err(unavailable)?,
                chapter_title: row.try_get("chapter_title").map_err(unavailable)?,
                version_id: row.try_get("version_id").map_err(unavailable)?,
                price_minor: row.try_get("price_amount").map_err(unavailable)?,
            })
        })
        .transpose()
    }

    /// Match the D checkout policy: a refunded purchase can be bought again
    /// only when the creator-posting/refund path is explicitly enabled.
    pub async fn purchased_chapter(
        &self,
        actor: i64,
        chapter_id: i64,
    ) -> Result<bool, CheckoutError> {
        if actor <= 0 || chapter_id <= 0 {
            return Ok(false);
        }
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM library_purchased_chapters
             WHERE user_id=$1 AND chapter_id=$2
               AND (NOT $3::BOOLEAN OR NOT EXISTS (
                 SELECT 1 FROM finance_direct_refund_postings refund
                 WHERE refund.purchase_id=library_purchased_chapters.purchase_id)))",
        )
        .bind(actor)
        .bind(chapter_id)
        .bind(self.creator_posting_enabled)
        .fetch_one(&self.pool)
        .await
        .map_err(unavailable)
    }

    /// Allow exact successful-command replays after V1 was replaced by V2.
    /// The D transaction still compares actor/key/chapter/version/price.
    /// A new command must name a currently published attached edition.
    pub async fn browser_offer_ready_or_replay(
        &self,
        actor: i64,
        chapter: i64,
        version: i64,
        price: i64,
        command_key: &str,
    ) -> Result<bool, CheckoutError> {
        if actor <= 0 || chapter <= 0 || version <= 0 || price <= 0 {
            return Ok(false);
        }
        let completed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM chapter_purchase_commands
             WHERE user_id=$1 AND command_key=$2 AND purchase_id IS NOT NULL)",
        )
        .bind(actor)
        .bind(command_key)
        .fetch_one(&self.pool)
        .await
        .map_err(unavailable)?;
        if completed {
            return Ok(true);
        }
        let offered = self.published_chapter_offer(chapter).await?;
        Ok(offered
            .is_some_and(|current| current.version_id == version && current.price_minor == price))
    }
}
