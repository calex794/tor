//! D-03: PostgreSQL authority for a directly purchased, pinned chapter version.
//! This implementation never reads progress, book visibility or a client version.
use crate::services::entitlements::{decide_direct_purchase, DirectPurchaseRecord};
use async_trait::async_trait;
use sqlx::{PgPool, Row};
use tor_contracts::paid_delivery::{
    Actor, DeliveryError, EntitlementDecision, EntitlementPort, PositiveId,
};

/// An owned, readable edition. Never contains paid text or signed object URLs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadablePurchase {
    pub purchase_id: i64,
    pub chapter_id: i64,
    pub version_id: i64,
    pub version_no: i32,
    pub chapter_no: i32,
    pub book_title: String,
    pub chapter_title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadablePurchasesPage {
    pub items: Vec<ReadablePurchase>,
    pub next_after: Option<i64>,
}

#[derive(Clone)]
pub struct PgDirectEntitlements {
    pool: PgPool,
}

impl PgDirectEntitlements {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// D-owned list for the authenticated reader. A purchase ID is only a
    /// navigation cursor, never an entitlement, and is never client-scoped.
    /// The same canonical SQL policy guards both this list and paid grants.
    pub async fn readable_purchases(
        &self,
        actor: PositiveId,
        before: Option<PositiveId>,
    ) -> Result<ReadablePurchasesPage, DeliveryError> {
        let mut rows = sqlx::query(
            "SELECT p.id AS purchase_id, p.chapter_id, p.version_id,
                    v.version_no, c.chapter_no,
                    b.title AS book_title, c.title AS chapter_title
             FROM library_purchased_chapters l
             JOIN chapter_purchase_facts p
               ON p.id=l.purchase_id AND p.user_id=l.user_id
              AND p.chapter_id=l.chapter_id AND p.version_id=l.active_version_id
             JOIN chapters c ON c.id=p.chapter_id
             JOIN books b ON b.id=c.book_id
             JOIN chapter_versions v ON v.chapter_id=p.chapter_id AND v.id=p.version_id
             WHERE l.user_id=$1
               AND ($2::BIGINT IS NULL OR p.id<$2)
               AND tor_purchase_version_readable($1,p.id,p.chapter_id,p.version_id)
             ORDER BY p.id DESC
             LIMIT 31",
        )
        .bind(actor.get())
        .bind(before.map(PositiveId::get))
        .fetch_all(&self.pool)
        .await
        .map_err(|_| DeliveryError::Unavailable)?;
        let more = rows.len() > 30;
        rows.truncate(30);
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(ReadablePurchase {
                purchase_id: row
                    .try_get("purchase_id")
                    .map_err(|_| DeliveryError::Unavailable)?,
                chapter_id: row
                    .try_get("chapter_id")
                    .map_err(|_| DeliveryError::Unavailable)?,
                version_id: row
                    .try_get("version_id")
                    .map_err(|_| DeliveryError::Unavailable)?,
                version_no: row
                    .try_get("version_no")
                    .map_err(|_| DeliveryError::Unavailable)?,
                chapter_no: row
                    .try_get("chapter_no")
                    .map_err(|_| DeliveryError::Unavailable)?,
                book_title: row
                    .try_get("book_title")
                    .map_err(|_| DeliveryError::Unavailable)?,
                chapter_title: row
                    .try_get("chapter_title")
                    .map_err(|_| DeliveryError::Unavailable)?,
            });
        }
        let next_after = if more {
            items.last().map(|row| row.purchase_id)
        } else {
            None
        };
        Ok(ReadablePurchasesPage { items, next_after })
    }
}

#[async_trait]
impl EntitlementPort for PgDirectEntitlements {
    async fn resolve_entitlement(
        &self,
        actor: Actor,
        chapter_id: PositiveId,
        purchase_id: Option<PositiveId>,
    ) -> Result<EntitlementDecision, DeliveryError> {
        // Optional purchase identity is only a filter: it never substitutes
        // for matching authenticated actor + confirmed non-revoked purchase.
        let row = sqlx::query(
            "SELECT l.active_version_id AS version_id,
                    (r.purchase_id IS NOT NULL) AS revoked,
                    (b.admin_hold OR c.admin_hold) AS admin_hold,
                    (b.is_deleted OR c.is_deleted) AS hard_deleted,
                    tor_purchase_version_readable(
                        $1, p.id, p.chapter_id, p.version_id
                    ) AS canonical_readable
             FROM library_purchased_chapters l
             JOIN chapter_purchase_facts p
               ON p.id=l.purchase_id AND p.user_id=l.user_id
              AND p.chapter_id=l.chapter_id AND p.version_id=l.active_version_id
             JOIN chapter_versions v
               ON v.id=p.version_id AND v.chapter_id=p.chapter_id
             JOIN chapters c ON c.id=p.chapter_id
             JOIN books b ON b.id=c.book_id
             LEFT JOIN chapter_purchase_revocations r ON r.purchase_id=p.id
             WHERE l.user_id=$1 AND l.chapter_id=$2
               AND ($3::BIGINT IS NULL OR p.id=$3)",
        )
        .bind(actor.user_id.get())
        .bind(chapter_id.get())
        .bind(purchase_id.map(PositiveId::get))
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| DeliveryError::Unavailable)?;
        let record = row
            .map(|row| {
                Ok(DirectPurchaseRecord {
                    purchased_version_id: row
                        .try_get("version_id")
                        .map_err(|_| DeliveryError::Unavailable)?,
                    revoked: row
                        .try_get("revoked")
                        .map_err(|_| DeliveryError::Unavailable)?,
                    admin_hold: row
                        .try_get("admin_hold")
                        .map_err(|_| DeliveryError::Unavailable)?,
                    hard_deleted: row
                        .try_get("hard_deleted")
                        .map_err(|_| DeliveryError::Unavailable)?,
                    canonical_readable: row
                        .try_get("canonical_readable")
                        .map_err(|_| DeliveryError::Unavailable)?,
                })
            })
            .transpose()?;
        Ok(decide_direct_purchase(chapter_id, record))
    }
}
