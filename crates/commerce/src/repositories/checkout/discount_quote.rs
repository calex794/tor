//! Private exact single-chapter offer, resolved by D, never by a browser.
use super::{discounts::eligible_discount, unavailable, PgCheckout};
use crate::domain::money::CheckoutError;
use sqlx::Row;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SingleChapterQuote {
    pub book_id: i64,
    pub chapter_id: i64,
    pub chapter_no: i32,
    pub chapter_title: String,
    pub book_title: String,
    pub version_id: i64,
    pub base_price_minor: i64,
    pub final_price_minor: i64,
    pub discount_percent: Option<i16>,
    pub requires_follow_days: Option<i16>,
}

impl PgCheckout {
    pub async fn quote_single(
        &self,
        actor: i64,
        book_id: i64,
        chapter_id: i64,
    ) -> Result<SingleChapterQuote, CheckoutError> {
        if actor <= 0 || book_id <= 0 || chapter_id <= 0 {
            return Err(CheckoutError::Inaccessible);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let offer = sqlx::query(
            "SELECT p.version_id,c.price_amount,c.chapter_no,c.title AS chapter_title,
                    b.id AS book_id,b.title AS book_title,b.owner_user_id
             FROM chapters AS c
             JOIN books AS b ON b.id=c.book_id
             JOIN chapter_publications AS p ON p.chapter_id=c.id
             JOIN chapter_versions AS v ON v.id=p.version_id AND v.chapter_id=c.id
             JOIN revenue_share_states AS st ON st.book_id=b.id
             WHERE c.id=$1 AND b.id=$2 AND c.visibility='public'
               AND c.access_level='paid' AND c.price_amount>0
               AND NOT c.admin_hold AND NOT c.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
               AND b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
               AND (b.owner_user_id IS NULL OR b.owner_user_id<>$3)
             FOR SHARE OF c,b,p",
        )
        .bind(chapter_id)
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?
        .ok_or(CheckoutError::Inaccessible)?;
        // Match the canonical checkout decision: an approved and settled
        // refund allows a new purchase only with D-08 creator postings.
        // The quote must never offer a checkout the real debit rejects.
        let owned = sqlx::query(
            "SELECT EXISTS (
                SELECT 1 FROM finance_direct_refund_postings AS refund
                WHERE refund.purchase_id=l.purchase_id
             ) AS settled_refund
             FROM library_purchased_chapters AS l
             WHERE l.user_id=$1 AND l.chapter_id=$2",
        )
        .bind(actor)
        .bind(chapter_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if let Some(owned) = owned {
            let refunded: bool = owned.try_get("settled_refund").map_err(unavailable)?;
            if !refunded {
                return Err(CheckoutError::AlreadyOwned);
            }
            if !self.creator_posting_enabled {
                return Err(CheckoutError::Unavailable);
            }
        }
        let base: i64 = offer.try_get("price_amount").map_err(unavailable)?;
        let discount = eligible_discount(
            &mut tx,
            actor,
            book_id,
            offer.try_get("owner_user_id").map_err(unavailable)?,
            base,
        )
        .await?;
        let quote = SingleChapterQuote {
            book_id,
            chapter_id,
            chapter_no: offer.try_get("chapter_no").map_err(unavailable)?,
            chapter_title: offer.try_get("chapter_title").map_err(unavailable)?,
            book_title: offer.try_get("book_title").map_err(unavailable)?,
            version_id: offer.try_get("version_id").map_err(unavailable)?,
            base_price_minor: base,
            final_price_minor: discount.as_ref().map_or(base, |d| d.paid_minor),
            discount_percent: discount.map(|d| d.discount_percent),
            requires_follow_days: discount.map(|d| d.min_follow_days),
        };
        tx.commit().await.map_err(unavailable)?;
        Ok(quote)
    }
}
