//! D-03: PostgreSQL authority for a directly purchased, pinned chapter version.
//! This implementation never reads progress, book visibility or a client version.
use crate::services::entitlements::{decide_direct_purchase, DirectPurchaseRecord};
use async_trait::async_trait;
use sqlx::{PgPool, Row};
use tor_contracts::paid_delivery::{
    Actor, DeliveryError, EntitlementDecision, EntitlementPort, PositiveId,
};

#[derive(Clone)]
pub struct PgDirectEntitlements {
    pool: PgPool,
}

impl PgDirectEntitlements {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
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
                    (b.is_deleted OR c.is_deleted) AS hard_deleted
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
                })
            })
            .transpose()?;
        Ok(decide_direct_purchase(chapter_id, record))
    }
}
