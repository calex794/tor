//! D-06: read-only refund preflight for evidenced Layer purchases.
//! A wallet debit, term revocation or non-empty reason is NOT enough to
//! establish the refund deadline, first-content exposure, or reversal authority.
use crate::services::refunds::{LayerRefundIntent, RefundGateError};
use sqlx::PgPool;

#[derive(Clone)]
pub struct PgLayerRefundPreflight {
    pool: PgPool,
}

impl PgLayerRefundPreflight {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Confirms actor/term ownership and immutable internal wallet evidence.
    /// All current S031 terms lack purchase-time refund_policy_version,
    /// refund_deadline_at and reliable first-exposure tracking. Therefore
    /// even a genuine payment must fail with PolicyUnavailable (future HTTP:
    /// 422 REFUND_POLICY_UNAVAILABLE). No money or access state is mutated.
    pub async fn inspect(&self, intent: &LayerRefundIntent) -> Result<(), RefundGateError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| RefundGateError::Unavailable)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(|_| RefundGateError::Unavailable)?;

        // The same D-05 evidence that guards checkout ties a paid Layer term
        // to the exact command, offer, RUB amount and immutable wallet entry.
        // Filter actor inside SQL to avoid revealing another user's term.
        let revoked: Option<bool> = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM commerce_layer_term_revocations r
                WHERE r.term_id=t.id
             )
             FROM commerce_layer_terms t
             WHERE t.id=$1 AND t.user_id=$2
               AND d05_layer_term_has_wallet_evidence(t.id)",
        )
        .bind(intent.term().get())
        .bind(intent.actor().get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| RefundGateError::Unavailable)?;

        tx.commit()
            .await
            .map_err(|_| RefundGateError::Unavailable)?;

        match revoked {
            None => Err(RefundGateError::NotFound),
            Some(true) => Err(RefundGateError::RevocationPresent),
            Some(false) => Err(RefundGateError::PolicyUnavailable),
        }
    }
}
