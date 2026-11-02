//! D-06: read-only direct chapter refund and swap candidate checks.
//!
//! This is *not* settlement authority. The Python NEwBOO 7-day window is
//! historical source behavior, NOT an approved TOR policy. TOR checkout currently
//! stores no approved refund terms, and main has no verified first paid-grant
//! barrier. Even a well-formed candidate is therefore explicitly fail-closed.
//! Do not issue money, revoke entitlements or expose apply endpoints from here.

use sqlx::{PgPool, Row};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectRefundGate {
    NotFound,
    Revoked,
    PolicyUnavailable,
    WindowExpired,
    SettlementUnverified,
    AlreadyRequested,
    AlreadyOpened,
    FirstGrantBarrierUnavailable,
    FirstGrantNotSeenInTrackedPurchase,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SwapQuoteError {
    NotFound,
    TargetNotAllowed,
    TargetAlreadyOwned,
    InvalidMoney,
    Overflow,
    InsufficientBalance,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectSwapQuote {
    pub source_purchase_id: i64,
    pub target_chapter_id: i64,
    pub target_version_id: i64,
    pub source_price_minor: i64,
    pub target_price_minor: i64,
    /// Positive = buyer owes the difference, negative = future wallet credit.
    pub delta_minor: i64,
    /// A price quote is NEVER a purchase, refund or authorization.
    pub gate: DirectRefundGate,
}

#[derive(Clone)]
pub struct PgDirectRefundPreflight {
    pool: PgPool,
}

impl PgDirectRefundPreflight {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// A single read-only snapshot ties owner, exact immutable purchase,
    /// ledger source, revocation and per-purchase policy together. Without a
    /// trusted first-grant cutover there is NO "unopened" finding.
    pub async fn refund_gate(&self, actor: i64, purchase_id: i64) -> DirectRefundGate {
        if actor <= 0 || purchase_id <= 0 {
            return DirectRefundGate::NotFound;
        }
        let row = sqlx::query(
            "SELECT p.refund_policy_version,p.first_grant_tracking_version,
                    EXISTS (
                        SELECT 1 FROM finance_direct_refund_policy_catalog c
                        WHERE c.version=p.refund_policy_version
                          AND c.approved_at<=p.acquired_at
                          AND c.effective_at<=p.acquired_at
                          AND p.refund_deadline_at<=p.acquired_at
                              + (c.window_seconds * INTERVAL '1 second')
                    ) AS approved_policy,
                    (p.refund_deadline_at IS NOT NULL
                     AND p.refund_deadline_at < now()) AS expired,
                    (r.purchase_id IS NOT NULL) AS revoked,
                    (SELECT COUNT(*) FROM reader_wallet_entries e
                     WHERE e.purchase_id=p.id AND e.user_id=p.user_id
                       AND e.kind='chapter_purchase'
                       AND e.delta_minor=-p.amount_minor
                       AND e.reference=p.settlement_reference) AS matching_debits,
                    EXISTS (
                      SELECT 1 FROM finance_direct_refund_requests q
                      WHERE q.purchase_id=p.id
                        AND q.state IN ('pending','processing')
                    ) AS already_requested,
                    EXISTS (
                      SELECT 1 FROM finance_chapter_postings fp
                      JOIN finance_platform_credits pc
                        ON pc.purchase_id=fp.purchase_id
                      WHERE fp.purchase_id=p.id
                        AND fp.gross_minor=p.amount_minor
                        AND pc.amount_minor=fp.platform_fee_minor
                        AND fp.refund_policy_version=p.refund_policy_version
                        AND fp.refund_deadline_at=p.refund_deadline_at
                        AND (SELECT COALESCE(SUM(ce.amount_minor),0)
                             FROM finance_creator_earnings ce
                             WHERE ce.purchase_id=p.id)=fp.creator_pool_minor
                    ) AS source_finance
             FROM chapter_purchase_facts p
             JOIN library_purchased_chapters l
               ON l.purchase_id=p.id AND l.user_id=p.user_id
              AND l.chapter_id=p.chapter_id
              AND l.active_version_id=p.version_id
             LEFT JOIN chapter_purchase_revocations r ON r.purchase_id=p.id
             WHERE p.id=$1 AND p.user_id=$2",
        )
        .bind(purchase_id)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await;
        let row = match row {
            Ok(Some(row)) => row,
            Ok(None) => return DirectRefundGate::NotFound,
            Err(_) => return DirectRefundGate::Unavailable,
        };
        let read_flag = |name: &str| row.try_get::<bool, _>(name).ok();
        if read_flag("revoked") == Some(true) {
            return DirectRefundGate::Revoked;
        }
        let policy = row.try_get::<Option<String>, _>("refund_policy_version");
        match policy {
            Ok(Some(ref version)) if !version.is_empty() => {}
            Ok(None) => return DirectRefundGate::PolicyUnavailable,
            _ => return DirectRefundGate::Unavailable,
        }
        if read_flag("approved_policy") != Some(true) {
            return DirectRefundGate::PolicyUnavailable;
        }
        if read_flag("expired") == Some(true) {
            return DirectRefundGate::WindowExpired;
        }
        if row.try_get::<i64, _>("matching_debits").ok() != Some(1)
            || read_flag("source_finance") != Some(true)
        {
            return DirectRefundGate::SettlementUnverified;
        }
        if read_flag("already_requested") == Some(true) {
            return DirectRefundGate::AlreadyRequested;
        }
        if read_flag("expired").is_none() || read_flag("already_requested").is_none() {
            return DirectRefundGate::Unavailable;
        }
        let opened = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                 SELECT 1 FROM chapter_purchase_first_exposures e
                 WHERE e.purchase_id=$1 AND e.user_id=$2
               )",
        )
        .bind(purchase_id)
        .bind(actor)
        .fetch_one(&self.pool)
        .await;
        match opened {
            Ok(true) => DirectRefundGate::AlreadyOpened,
            // A missing row does NOT prove an unopened historic purchase:
            // approval stays unavailable until a known first-grant cutover.
            Ok(false) => match row.try_get::<Option<String>, _>("first_grant_tracking_version") {
                Ok(Some(version)) if version == "c06-first-issue-v1" => {
                    // A safe classification for review, not finance approval.
                    DirectRefundGate::FirstGrantNotSeenInTrackedPurchase
                }
                Ok(None) => DirectRefundGate::FirstGrantBarrierUnavailable,
                _ => DirectRefundGate::Unavailable,
            },
            Err(_) => DirectRefundGate::Unavailable,
        }
    }

    /// UI-only price comparison. A valid quote MUST NOT be treated as
    /// permission to exchange; the returned gate always prevents apply until
    /// the approved policy, exposure barrier and serializable posting exist.
    pub async fn swap_quote(
        &self,
        actor: i64,
        source_purchase_id: i64,
        target_chapter_id: i64,
    ) -> Result<DirectSwapQuote, SwapQuoteError> {
        if actor <= 0 || source_purchase_id <= 0 || target_chapter_id <= 0 {
            return Err(SwapQuoteError::NotFound);
        }
        let source = sqlx::query(
            "SELECT p.chapter_id, p.amount_minor, c.book_id,
                    b.owner_user_id
             FROM chapter_purchase_facts p
             JOIN chapters c ON c.id=p.chapter_id
             JOIN books b ON b.id=c.book_id
             JOIN library_purchased_chapters l
               ON l.purchase_id=p.id AND l.user_id=p.user_id
              AND l.chapter_id=p.chapter_id
              AND l.active_version_id=p.version_id
             WHERE p.id=$1 AND p.user_id=$2
               AND NOT EXISTS (
                 SELECT 1 FROM chapter_purchase_revocations r
                 WHERE r.purchase_id=p.id
               )",
        )
        .bind(source_purchase_id)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| SwapQuoteError::Unavailable)?
        .ok_or(SwapQuoteError::NotFound)?;
        let owner: Option<i64> = source
            .try_get("owner_user_id")
            .map_err(|_| SwapQuoteError::Unavailable)?;
        let source_book_id: i64 = source
            .try_get("book_id")
            .map_err(|_| SwapQuoteError::Unavailable)?;
        if owner.is_none()
            || source
                .try_get::<i64, _>("chapter_id")
                .map_err(|_| SwapQuoteError::Unavailable)?
                == target_chapter_id
        {
            return Err(SwapQuoteError::TargetNotAllowed);
        }
        // NEwBOO web policy: another published paid Book of the same owner,
        // never a chapter already owned by the buyer.
        let target = sqlx::query(
            "SELECT c.price_amount, pub.version_id
             FROM chapters c
             JOIN books b ON b.id=c.book_id
             JOIN chapter_publications pub ON pub.chapter_id=c.id
             JOIN chapter_versions v
               ON v.chapter_id=c.id AND v.id=pub.version_id
             WHERE c.id=$1 AND c.book_id<>$2 AND b.owner_user_id=$3
               AND c.access_level='paid' AND c.visibility='public'
               AND c.price_amount>0 AND NOT c.admin_hold AND NOT c.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
               AND b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted",
        )
        .bind(target_chapter_id)
        .bind(source_book_id)
        .bind(owner)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| SwapQuoteError::Unavailable)?
        .ok_or(SwapQuoteError::TargetNotAllowed)?;
        let owned: Option<i64> = sqlx::query_scalar(
            "SELECT purchase_id FROM library_purchased_chapters
             WHERE user_id=$1 AND chapter_id=$2",
        )
        .bind(actor)
        .bind(target_chapter_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| SwapQuoteError::Unavailable)?;
        if owned.is_some() {
            return Err(SwapQuoteError::TargetAlreadyOwned);
        }
        let original: i64 = source
            .try_get("amount_minor")
            .map_err(|_| SwapQuoteError::Unavailable)?;
        let price: i64 = target
            .try_get("price_amount")
            .map_err(|_| SwapQuoteError::Unavailable)?;
        let balance: i64 = sqlx::query_scalar(
            "SELECT balance_minor FROM reader_wallet_accounts
             WHERE user_id=$1 AND currency='RUB'",
        )
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| SwapQuoteError::Unavailable)?
        .unwrap_or(0);
        let delta = checked_swap_delta(original, price, balance)?;
        Ok(DirectSwapQuote {
            source_purchase_id,
            target_chapter_id,
            target_version_id: target
                .try_get("version_id")
                .map_err(|_| SwapQuoteError::Unavailable)?,
            source_price_minor: original,
            target_price_minor: price,
            delta_minor: delta,
            gate: self.refund_gate(actor, source_purchase_id).await,
        })
    }
}

pub fn checked_swap_delta(
    source_minor: i64,
    target_minor: i64,
    buyer_balance_minor: i64,
) -> Result<i64, SwapQuoteError> {
    if source_minor <= 0 || target_minor <= 0 || buyer_balance_minor < 0 {
        return Err(SwapQuoteError::InvalidMoney);
    }
    let delta = target_minor
        .checked_sub(source_minor)
        .ok_or(SwapQuoteError::Overflow)?;
    if delta > buyer_balance_minor {
        return Err(SwapQuoteError::InsufficientBalance);
    }
    Ok(delta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d_06_swap_delta_conserves_minor_units_and_denies_overdraft() {
        assert_eq!(checked_swap_delta(100, 150, 50), Ok(50));
        assert_eq!(checked_swap_delta(200, 150, 0), Ok(-50));
        assert_eq!(checked_swap_delta(100, 100, 0), Ok(0));
        assert_eq!(
            checked_swap_delta(100, 150, 49),
            Err(SwapQuoteError::InsufficientBalance)
        );
        assert_eq!(
            checked_swap_delta(0, 100, 100),
            Err(SwapQuoteError::InvalidMoney)
        );
        assert_eq!(checked_swap_delta(i64::MAX, 1, 0), Ok(1 - i64::MAX));
    }
}
