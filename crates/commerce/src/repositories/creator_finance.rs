//! S038 / FIN-02: NEwBOO-compatible unified wallet payout capacity.
//! This module never transfers money, debits a wallet or confirms a payout.
#![forbid(unsafe_code)]

use crate::domain::money::IdempotencyKey;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreatorFinanceError {
    Invalid,
    Forbidden,
    Conflict,
    Insufficient,
    RateLimited,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreatorFinanceTotals {
    pub wallet_balance_minor: i64,
    pub combined_balance_minor: i64,
    pub gross_minor: i64,
    pub reversed_minor: i64,
    pub immature_minor: i64,
    pub mature_minor: i64,
    pub open_refund_hold_minor: i64,
    pub reserved_minor: i64,
    pub available_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatorPayoutRequest {
    pub id: i64,
    pub amount_minor: i64,
    pub state: String,
    pub created_utc: String,
}

/// Read-only details of a creator's own immutable purchase-origin earning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatorEarningLine {
    pub purchase_id: i64,
    pub amount_minor: i64,
    pub share_percent: i16,
    pub reversed_minor: i64,
    pub status: String,
    pub earned_utc: String,
}

/// One append-only reversal of an earning originally belonging to this actor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatorRefundLine {
    pub purchase_id: i64,
    pub amount_minor: i64,
    pub reversed_utc: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PayoutReservationReceipt {
    pub id: i64,
    pub created: bool,
}

#[derive(Clone)]
pub struct PgCreatorFinance {
    pool: PgPool,
}

const BALANCE_SQL: &str = r#"
    WITH net AS (
      SELECT e.amount_minor,
             (e.amount_minor-COALESCE(r.amount_minor,0)
                            -COALESCE(c.amount_minor,0))::BIGINT AS net_minor,
             COALESCE(r.amount_minor,0) AS reversed_minor,
             COALESCE(c.amount_minor,0) AS credited_minor,
             e.clear_at,
             EXISTS (
                SELECT 1 FROM finance_direct_refund_requests q
                WHERE q.purchase_id=e.purchase_id
                  AND q.state IN ('pending','processing','approved')
                  AND r.purchase_id IS NULL
             ) AS refund_hold
      FROM finance_creator_earnings e
      LEFT JOIN finance_direct_refund_creator_reversals r
        ON r.purchase_id=e.purchase_id AND r.user_id=e.user_id
      LEFT JOIN finance_creator_wallet_credits c
        ON c.purchase_id=e.purchase_id AND c.user_id=e.user_id
      WHERE e.user_id=$1 AND e.origin_type='chapter_purchase'
    ), totals AS (
      SELECT COALESCE(SUM(amount_minor),0)::BIGINT AS gross,
             COALESCE(SUM(reversed_minor),0)::BIGINT AS reversed,
             COALESCE(SUM(credited_minor),0)::BIGINT AS credited,
             COALESCE(SUM(CASE WHEN clear_at>clock_timestamp()
                              THEN GREATEST(net_minor,0) ELSE 0 END),0)::BIGINT AS immature,
             COALESCE(SUM(CASE WHEN clear_at<=clock_timestamp()
                              THEN net_minor ELSE 0 END),0)::BIGINT AS mature,
             COALESCE(SUM(CASE WHEN clear_at<=clock_timestamp() AND refund_hold
                              THEN GREATEST(net_minor,0) ELSE 0 END),0)::BIGINT AS refund_hold
      FROM net
    ), reservations AS (
      SELECT COALESCE(SUM(reservation_minor),0)::BIGINT AS reserved
      FROM finance_creator_payout_requests
      WHERE user_id=$1 AND state='pending'
    )
    SELECT w.wallet_balance,
           (w.wallet_balance+t.gross-t.reversed-t.credited)::BIGINT AS combined_balance,
           t.gross,t.reversed,t.immature,t.mature,t.refund_hold,r.reserved,
           GREATEST(0,w.wallet_balance+t.mature-t.refund_hold-r.reserved)::BIGINT
             AS available
    FROM totals t CROSS JOIN reservations r
    CROSS JOIN (
       SELECT COALESCE(
         (SELECT balance_minor FROM reader_wallet_accounts
          WHERE user_id=$1 AND currency='RUB'),0)::BIGINT AS wallet_balance
    ) w
"#;

impl PgCreatorFinance {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn totals_in(
        tx: &mut Transaction<'_, Postgres>,
        user_id: i64,
    ) -> Result<CreatorFinanceTotals, CreatorFinanceError> {
        let row: (i64, i64, i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(BALANCE_SQL)
            .bind(user_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(|_| CreatorFinanceError::Unavailable)?;
        Ok(CreatorFinanceTotals {
            wallet_balance_minor: row.0,
            combined_balance_minor: row.1,
            gross_minor: row.2,
            reversed_minor: row.3,
            immature_minor: row.4,
            mature_minor: row.5,
            open_refund_hold_minor: row.6,
            reserved_minor: row.7,
            available_minor: row.8,
        })
    }

    /// NEwBOO financial statement: unified wallet value plus creator credits,
    /// including funds introduced through verified wallet funding. Historical
    /// creator earnings remain visible after team membership changes.
    pub async fn totals(&self, user_id: i64) -> Result<CreatorFinanceTotals, CreatorFinanceError> {
        if user_id <= 0 {
            return Err(CreatorFinanceError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| CreatorFinanceError::Unavailable)?;
        let totals = Self::totals_in(&mut tx, user_id).await?;
        tx.commit()
            .await
            .map_err(|_| CreatorFinanceError::Unavailable)?;
        Ok(totals)
    }

    pub async fn history(
        &self,
        user_id: i64,
    ) -> Result<Vec<CreatorPayoutRequest>, CreatorFinanceError> {
        if user_id <= 0 {
            return Err(CreatorFinanceError::Invalid);
        }
        let rows: Vec<(i64, i64, String, String)> = sqlx::query_as(
            "SELECT id,amount_minor,state,
                    to_char(created_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI')
             FROM finance_creator_payout_requests
             WHERE user_id=$1 ORDER BY id DESC LIMIT 30",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| CreatorFinanceError::Unavailable)?;
        Ok(rows
            .into_iter()
            .map(
                |(id, amount_minor, state, created_utc)| CreatorPayoutRequest {
                    id,
                    amount_minor,
                    state,
                    created_utc,
                },
            )
            .collect())
    }

    /// NEwBOO finance parity: per-purchase earning history, not only totals.
    /// Returning a source purchase identifier does not disclose the buyer,
    /// book text or another collaborator's earning share.
    pub async fn recent_earnings(
        &self,
        user_id: i64,
    ) -> Result<Vec<CreatorEarningLine>, CreatorFinanceError> {
        if user_id <= 0 {
            return Err(CreatorFinanceError::Invalid);
        }
        let rows: Vec<(i64, i64, i16, i64, String, String)> = sqlx::query_as(
            "SELECT e.purchase_id,e.amount_minor,e.share_percent,
                    COALESCE(r.amount_minor,0)::BIGINT,
                    CASE WHEN r.purchase_id IS NOT NULL THEN 'reversed'
                         WHEN EXISTS (
                           SELECT 1 FROM finance_direct_refund_requests q
                           WHERE q.purchase_id=e.purchase_id
                             AND q.state IN ('pending','processing')
                         ) THEN 'refund_hold'
                         WHEN e.clear_at>clock_timestamp() THEN 'immature'
                         ELSE 'mature' END,
                    to_char(e.created_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI')
             FROM finance_creator_earnings e
             LEFT JOIN finance_direct_refund_creator_reversals r
               ON r.purchase_id=e.purchase_id AND r.user_id=e.user_id
             WHERE e.user_id=$1 AND e.origin_type='chapter_purchase'
             ORDER BY e.created_at DESC,e.purchase_id DESC LIMIT 50",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| CreatorFinanceError::Unavailable)?;
        Ok(rows
            .into_iter()
            .map(
                |(purchase_id, amount_minor, share_percent, reversed_minor, status, earned_utc)| {
                    CreatorEarningLine {
                        purchase_id,
                        amount_minor,
                        share_percent,
                        reversed_minor,
                        status,
                        earned_utc,
                    }
                },
            )
            .collect())
    }

    /// Historical refunds remain visible after team/owner changes.
    /// Query is scoped to the actual earning's recipient, not buyer or
    /// current book membership; no buyer identity is selected.
    pub async fn recent_refunds(
        &self,
        user_id: i64,
    ) -> Result<Vec<CreatorRefundLine>, CreatorFinanceError> {
        if user_id <= 0 {
            return Err(CreatorFinanceError::Invalid);
        }
        let rows: Vec<(i64, i64, String)> = sqlx::query_as(
            "SELECT r.purchase_id,r.amount_minor,
                    to_char(r.created_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI')
             FROM finance_direct_refund_creator_reversals r
             JOIN finance_creator_earnings e
               ON e.purchase_id=r.purchase_id AND e.user_id=r.user_id
             WHERE r.user_id=$1
             ORDER BY r.created_at DESC,r.purchase_id DESC LIMIT 50",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| CreatorFinanceError::Unavailable)?;
        Ok(rows
            .into_iter()
            .map(
                |(purchase_id, amount_minor, reversed_utc)| CreatorRefundLine {
                    purchase_id,
                    amount_minor,
                    reversed_utc,
                },
            )
            .collect())
    }

    /// Atomically claim a single internal pending reservation.
    ///
    /// Lock the owner account, not their current memberships: all concurrent
    /// requests from different app instances serialize on that same PG row.
    /// Same key+payload replays exactly; key with different amount conflicts.
    /// No approval, wallet debit, external provider or terminal payout occurs.
    pub async fn reserve_once(
        &self,
        actor: i64,
        amount_minor: i64,
        key: &IdempotencyKey,
    ) -> Result<PayoutReservationReceipt, CreatorFinanceError> {
        if actor <= 0 || !(1..=1_000_000_000_000).contains(&amount_minor) {
            return Err(CreatorFinanceError::Invalid);
        }
        let mut hash = Sha256::new();
        hash.update(b"tor.s038.unified.wallet.payout.pending.v1");
        hash.update(actor.to_be_bytes());
        hash.update(amount_minor.to_be_bytes());
        let digest = hash.finalize();

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| CreatorFinanceError::Unavailable)?;
        let active: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM users WHERE id=$1 AND account_state='active' FOR UPDATE",
        )
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| CreatorFinanceError::Unavailable)?;
        if active.is_none() {
            return Err(CreatorFinanceError::Forbidden);
        }
        // Match NEwBOO's current-participant access check for NEW requests,
        // without concealing historical balances and statements from former members.
        let participant: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM books b WHERE b.owner_user_id=$1
                UNION ALL
                SELECT 1 FROM book_members m WHERE m.user_id=$1 AND m.is_active
             )",
        )
        .bind(actor)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| CreatorFinanceError::Unavailable)?;
        if !participant {
            return Err(CreatorFinanceError::Forbidden);
        }

        let prior: Option<(i64, i64, Vec<u8>)> = sqlx::query_as(
            "SELECT id,amount_minor,intent_digest
             FROM finance_creator_payout_requests
             WHERE user_id=$1 AND intent_key=$2",
        )
        .bind(actor)
        .bind(key.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| CreatorFinanceError::Unavailable)?;
        if let Some((id, old_amount, old_digest)) = prior {
            if old_amount != amount_minor || old_digest.as_slice() != digest.as_slice() {
                return Err(CreatorFinanceError::Conflict);
            }
            tx.commit()
                .await
                .map_err(|_| CreatorFinanceError::Unavailable)?;
            return Ok(PayoutReservationReceipt { id, created: false });
        }

        let pending: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)::BIGINT FROM finance_creator_payout_requests
             WHERE user_id=$1 AND state='pending'",
        )
        .bind(actor)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| CreatorFinanceError::Unavailable)?;
        if pending >= 20 {
            return Err(CreatorFinanceError::RateLimited);
        }
        let totals = Self::totals_in(&mut tx, actor).await?;
        if amount_minor > totals.available_minor {
            return Err(CreatorFinanceError::Insufficient);
        }
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO finance_creator_payout_requests
             (user_id,amount_minor,intent_key,intent_digest,reservation_minor)
             VALUES($1,$2,$3,$4,$2) RETURNING id",
        )
        .bind(actor)
        .bind(amount_minor)
        .bind(key.as_str())
        .bind(digest.as_slice())
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| CreatorFinanceError::Unavailable)?;
        tx.commit()
            .await
            .map_err(|_| CreatorFinanceError::Unavailable)?;
        Ok(PayoutReservationReceipt { id, created: true })
    }
}
