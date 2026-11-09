//! S039/MONEY-03: advisory approval quote with the NEwBOO own-reserve rule.
//! Explicitly NOT an approval, wallet debit, or external settlement proof.
use super::{PayoutAdminError, PgCreatorPayoutAdmin};
use crate::repositories::creator_finance::PgCreatorFinance;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayoutApprovalQuote {
    pub request_id: i64,
    pub user_id: i64,
    pub amount_minor: i64,
    pub other_reserved_minor: i64,
    pub available_excluding_own_minor: i64,
    pub sufficient: bool,
}

impl PgCreatorPayoutAdmin {
    /// PostgreSQL-verified administrator, live recipient, and owner-level lock
    /// shared with reserve_once. This is only a snapshot; a future approval
    /// must recheck eligibility and debit+ledger in its OWN transaction.
    pub async fn approval_readiness(
        &self,
        session_digest: &[u8; 32],
        request_id: i64,
    ) -> Result<PayoutApprovalQuote, PayoutAdminError> {
        if request_id <= 0 {
            return Err(PayoutAdminError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| PayoutAdminError::Unavailable)?;
        let admin = Self::active_administrator(&mut tx, session_digest).await?;
        // Resolve recipient without locking payout first: new payout requests
        // serialize on the recipient account and this keeps lock order stable.
        let recipient: Option<i64> =
            sqlx::query_scalar("SELECT user_id FROM finance_creator_payout_requests WHERE id=$1")
                .bind(request_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|_| PayoutAdminError::Unavailable)?;
        let recipient = recipient.ok_or(PayoutAdminError::NotFound)?;
        let active: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM users
             WHERE id=$1 AND account_state='active' FOR NO KEY UPDATE",
        )
        .bind(recipient)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| PayoutAdminError::Unavailable)?;
        if active.is_none() {
            return Err(PayoutAdminError::Conflict);
        }
        let pending: Option<(i64, String)> = sqlx::query_as(
            "SELECT amount_minor,state
             FROM finance_creator_payout_requests
             WHERE id=$1 AND user_id=$2 FOR SHARE",
        )
        .bind(request_id)
        .bind(recipient)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| PayoutAdminError::Unavailable)?;
        let (amount, state) = pending.ok_or(PayoutAdminError::NotFound)?;
        if state != "pending" {
            return Err(PayoutAdminError::Conflict);
        }
        // The shared S038 totals already subtract ALL pending reservations.
        // Exclude this request's reservation exactly once. Reconstruct the
        // signed net BEFORE clamping: max(0,net)+own would overstate capacity
        // when clawback debt or holds make net negative.
        let totals = PgCreatorFinance::totals_in(&mut tx, recipient)
            .await
            .map_err(|_| PayoutAdminError::Unavailable)?;
        let other_reserved = totals
            .reserved_minor
            .checked_sub(amount)
            .filter(|v| *v >= 0)
            .ok_or(PayoutAdminError::Unavailable)?;
        let raw = totals
            .wallet_balance_minor
            .checked_add(totals.mature_minor)
            .and_then(|v| v.checked_sub(totals.open_refund_hold_minor))
            .and_then(|v| v.checked_sub(other_reserved))
            .ok_or(PayoutAdminError::Unavailable)?;
        let available = raw.max(0);
        let quote = PayoutApprovalQuote {
            request_id,
            user_id: recipient,
            amount_minor: amount,
            other_reserved_minor: other_reserved,
            available_excluding_own_minor: available,
            sufficient: available >= amount,
        };
        sqlx::query(
            "INSERT INTO finance_creator_payout_approval_read_audit
             (admin_user_id,payout_request_id,amount_minor,other_reserved_minor,
              available_excluding_own_minor,sufficient)
             VALUES($1,$2,$3,$4,$5,$6)",
        )
        .bind(admin)
        .bind(request_id)
        .bind(amount)
        .bind(other_reserved)
        .bind(available)
        .bind(quote.sufficient)
        .execute(&mut *tx)
        .await
        .map_err(|_| PayoutAdminError::Unavailable)?;
        tx.commit()
            .await
            .map_err(|_| PayoutAdminError::Unavailable)?;
        Ok(quote)
    }
}
