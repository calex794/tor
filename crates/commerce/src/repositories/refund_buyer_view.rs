//! Read-only, buyer-scoped D-06 refund request history and purchase choices.
//! This is not an eligibility decision or an instruction to move money.
use sqlx::{PgPool, Row};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuyerRefundPurchase {
    pub purchase_id: i64,
    pub book_title: String,
    pub chapter_title: String,
    pub amount_minor: i64,
    pub request_id: Option<i64>,
    pub request_state: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefundBuyerViewError {
    InvalidActor,
    Unavailable,
}

#[derive(Clone)]
pub struct PgRefundBuyerView {
    pool: PgPool,
}

impl PgRefundBuyerView {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Every row is owned by the authenticated actor. Include historical
    /// purchases even when the purchased entitlement has been revoked.
    /// No arbitrary pagination can reveal another account's history.
    pub async fn recent_for_buyer(
        &self,
        actor: i64,
    ) -> Result<Vec<BuyerRefundPurchase>, RefundBuyerViewError> {
        if actor <= 0 {
            return Err(RefundBuyerViewError::InvalidActor);
        }
        let rows = sqlx::query(
            "SELECT p.id AS purchase_id,b.title AS book_title,
                    c.title AS chapter_title,p.amount_minor,
                    r.id AS request_id,r.state AS request_state
             FROM chapter_purchase_facts p
             JOIN chapters c ON c.id=p.chapter_id
             JOIN books b ON b.id=c.book_id
             LEFT JOIN LATERAL (
                 SELECT q.id,q.state FROM finance_direct_refund_requests q
                 WHERE q.purchase_id=p.id AND q.user_id=p.user_id
                 ORDER BY q.id DESC LIMIT 1
             ) r ON true
             WHERE p.user_id=$1
             ORDER BY p.id DESC LIMIT 30",
        )
        .bind(actor)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| RefundBuyerViewError::Unavailable)?;
        rows.into_iter()
            .map(|row| {
                Ok(BuyerRefundPurchase {
                    purchase_id: row
                        .try_get("purchase_id")
                        .map_err(|_| RefundBuyerViewError::Unavailable)?,
                    book_title: row
                        .try_get("book_title")
                        .map_err(|_| RefundBuyerViewError::Unavailable)?,
                    chapter_title: row
                        .try_get("chapter_title")
                        .map_err(|_| RefundBuyerViewError::Unavailable)?,
                    amount_minor: row
                        .try_get("amount_minor")
                        .map_err(|_| RefundBuyerViewError::Unavailable)?,
                    request_id: row
                        .try_get("request_id")
                        .map_err(|_| RefundBuyerViewError::Unavailable)?,
                    request_state: row
                        .try_get("request_state")
                        .map_err(|_| RefundBuyerViewError::Unavailable)?,
                })
            })
            .collect()
    }
}
