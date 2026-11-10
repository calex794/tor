//! Bring mature, source-backed creator earnings into the unified wallet.
 //! Only called with the owner wallet row held FOR UPDATE by the posting tx.
use super::PayoutPostError;
use sqlx::{Postgres, Transaction};

// A bounded single transaction. Larger earners can settle additional mature
// credits with the existing idempotent S038 batch service before retrying.
const MAX_BATCHES: usize = 20;
const BATCH_SIZE: i64 = 20;

pub(super) async fn materialize_until(
    tx: &mut Transaction<'_, Postgres>,
    actor: i64,
    target_minor: i64,
    mut balance_minor: i64,
) -> Result<i64, PayoutPostError> {
    for _ in 0..MAX_BATCHES {
        if balance_minor >= target_minor {
            return Ok(balance_minor);
        }
        // Lock the exact immutable earning and purchase evidence after the
        // owner wallet. No pending refund, reversal or prior credit is eligible.
        let sources: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT e.purchase_id,e.amount_minor
             FROM finance_creator_earnings e
             JOIN chapter_purchase_facts p ON p.id=e.purchase_id
             WHERE e.user_id=$1 AND e.origin_type='chapter_purchase'
               AND e.clear_at<=clock_timestamp()
               AND p.refund_deadline_at<=clock_timestamp()
               AND NOT EXISTS (
                 SELECT 1 FROM finance_creator_wallet_credits c
                 WHERE c.purchase_id=e.purchase_id AND c.user_id=e.user_id
               )
               AND NOT EXISTS (
                 SELECT 1 FROM finance_direct_refund_requests q
                 WHERE q.purchase_id=e.purchase_id
                   AND q.state IN ('pending','processing','approved')
               )
               AND NOT EXISTS (
                 SELECT 1 FROM finance_direct_refund_creator_reversals r
                 WHERE r.purchase_id=e.purchase_id AND r.user_id=e.user_id
               )
               AND NOT EXISTS (
                 SELECT 1 FROM finance_direct_swap_creator_reversals sr
                 WHERE sr.source_purchase_id=e.purchase_id AND sr.user_id=e.user_id
               )
             ORDER BY e.purchase_id LIMIT $2 FOR UPDATE OF p,e",
        )
        .bind(actor)
        .bind(BATCH_SIZE)
        .fetch_all(&mut **tx)
        .await
        .map_err(|_| PayoutPostError::Unavailable)?;
        if sources.is_empty() {
            break;
        }
        for (purchase_id, amount) in sources {
            if amount <= 0 {
                return Err(PayoutPostError::Unavailable);
            }
            let checked = balance_minor
                .checked_add(amount)
                .ok_or(PayoutPostError::Unavailable)?;
            let current: i64 = sqlx::query_scalar(
                "UPDATE reader_wallet_accounts
                 SET balance_minor=balance_minor+$2
                 WHERE user_id=$1 AND currency='RUB'
                 RETURNING balance_minor",
            )
            .bind(actor)
            .bind(amount)
            .fetch_one(&mut **tx)
            .await
            .map_err(|_| PayoutPostError::Unavailable)?;
            if current != checked {
                return Err(PayoutPostError::Unavailable);
            }
            let reference = format!("creator-credit:{purchase_id}:{actor}");
            let journal_id: i64 = sqlx::query_scalar(
                "INSERT INTO reader_wallet_entries
                 (user_id,kind,delta_minor,balance_after_minor,reference)
                 VALUES($1,'creator_earning_credit',$2,$3,$4)
                 RETURNING id",
            )
            .bind(actor)
            .bind(amount)
            .bind(current)
            .bind(&reference)
            .fetch_one(&mut **tx)
            .await
            .map_err(|_| PayoutPostError::Unavailable)?;
            sqlx::query(
                "INSERT INTO finance_creator_wallet_credits
                 (purchase_id,user_id,amount_minor,wallet_entry_id)
                 VALUES($1,$2,$3,$4)",
            )
            .bind(purchase_id)
            .bind(actor)
            .bind(amount)
            .bind(journal_id)
            .execute(&mut **tx)
            .await
            .map_err(|_| PayoutPostError::Unavailable)?;
            balance_minor = current;
            if balance_minor >= target_minor {
                return Ok(balance_minor);
            }
        }
    }
    if balance_minor < target_minor {
        return Err(PayoutPostError::Insufficient);
    }
    Ok(balance_minor)
}
