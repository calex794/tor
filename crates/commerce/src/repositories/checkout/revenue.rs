//! D-08: versioned money terms and creator credits in a buyer purchase transaction.
//! No default revenue rate or refund window. The caller must explicitly enable
//! the writer AND the book must have operator-published finance terms.

use crate::domain::{
    money::{CheckoutError, MoneyMinor},
    revenue_allocation::{
        calculate_purchase_allocation, PurchaseAllocation, RevenueAllocationPolicy, RevenueShare,
    },
};
use sqlx::{Postgres, Row, Transaction};
use tor_contracts::paid_delivery::PositiveId;

pub(super) struct FinanceTerms {
    pub(super) allocation: PurchaseAllocation,
    pub(super) refund_policy_version: String,
    pub(super) refund_window_seconds: i64,
}

/// Read and lock one published terms revision and the frozen B-10 share lines.
/// No current member percentages are ever recomputed for an old purchase.
pub(super) async fn load_terms(
    tx: &mut Transaction<'_, Postgres>,
    book_id: i64,
    share_revision: i64,
    price_minor: i64,
) -> Result<FinanceTerms, CheckoutError> {
    let terms = sqlx::query(
        "SELECT t.policy_version,t.platform_fee_bps,
                t.refund_policy_version,t.refund_window_seconds
         FROM finance_revenue_term_states st
         JOIN finance_revenue_terms t
           ON t.book_id=st.book_id AND t.policy_version=st.policy_version
         WHERE st.book_id=$1 FOR SHARE OF st,t",
    )
    .bind(book_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| CheckoutError::Unavailable)?
    .ok_or(CheckoutError::Unavailable)?;
    let version: String = terms
        .try_get("policy_version")
        .map_err(|_| CheckoutError::Unavailable)?;
    let bps: i16 = terms
        .try_get("platform_fee_bps")
        .map_err(|_| CheckoutError::Unavailable)?;
    let policy = RevenueAllocationPolicy::new(
        version,
        u16::try_from(bps).map_err(|_| CheckoutError::Unavailable)?,
    )
    .map_err(|_| CheckoutError::Unavailable)?;
    let rows = sqlx::query(
        "SELECT user_id,share_percent FROM revenue_share_assignments
         WHERE book_id=$1 AND revision=$2 ORDER BY user_id",
    )
    .bind(book_id)
    .bind(share_revision)
    .fetch_all(&mut **tx)
    .await
    .map_err(|_| CheckoutError::Unavailable)?;
    let mut shares = Vec::with_capacity(rows.len());
    for row in rows {
        let id: i64 = row.try_get("user_id").map_err(|_| CheckoutError::Unavailable)?;
        let percent: i16 = row
            .try_get("share_percent")
            .map_err(|_| CheckoutError::Unavailable)?;
        shares.push(RevenueShare {
            recipient_id: PositiveId::new(id).map_err(|_| CheckoutError::Unavailable)?,
            share_percent: u8::try_from(percent).map_err(|_| CheckoutError::Unavailable)?,
        });
    }
    let allocation = calculate_purchase_allocation(
        MoneyMinor::new(price_minor).map_err(|_| CheckoutError::Unavailable)?,
        &policy,
        &shares,
    )
    .map_err(|_| CheckoutError::Unavailable)?;
    Ok(FinanceTerms {
        allocation,
        refund_policy_version: terms
            .try_get("refund_policy_version")
            .map_err(|_| CheckoutError::Unavailable)?,
        refund_window_seconds: terms
            .try_get("refund_window_seconds")
            .map_err(|_| CheckoutError::Unavailable)?,
    })
}

/// The caller's existing SQLx transaction already owns buyer debit, purchase,
/// library pin, exact B-10 share snapshot and reader_wallet_entries. A failure
/// here aborts them ALL. The deferred PostgreSQL trigger checks every credit
/// against the same immutable financial and B-10 evidence at COMMIT.
pub(super) async fn post_purchase(
    tx: &mut Transaction<'_, Postgres>,
    purchase_id: i64,
    book_id: i64,
    share_revision: i64,
    terms: &FinanceTerms,
) -> Result<(), CheckoutError> {
    let a = &terms.allocation;
    sqlx::query(
        "INSERT INTO finance_chapter_postings
          (purchase_id,book_id,share_revision,policy_version,
           refund_policy_version,refund_deadline_at,
           gross_minor,platform_fee_minor,creator_pool_minor)
         SELECT p.id,$2,$3,$4,p.refund_policy_version,p.refund_deadline_at,
                p.amount_minor,$5,$6
         FROM chapter_purchase_facts p WHERE p.id=$1",
    )
    .bind(purchase_id)
    .bind(book_id)
    .bind(share_revision)
    .bind(&a.policy_version)
    .bind(a.platform_fee_minor.get())
    .bind(a.creator_pool_minor.get())
    .execute(&mut **tx)
    .await
    .map_err(|_| CheckoutError::Unavailable)?;

    sqlx::query(
        "INSERT INTO finance_platform_credits(purchase_id,amount_minor)
         VALUES($1,$2)",
    )
    .bind(purchase_id)
    .bind(a.platform_fee_minor.get())
    .execute(&mut **tx)
    .await
    .map_err(|_| CheckoutError::Unavailable)?;

    // Use the original purchase's persisted refund deadline, not another
    // independent now()+7d calculation after committing the buyer charge.
    let deadline: Option<i64> = sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM refund_deadline_at)::BIGINT
         FROM chapter_purchase_facts WHERE id=$1",
    )
    .bind(purchase_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| CheckoutError::Unavailable)?;
    let holds = a
        .creator_holds(
            PositiveId::new(purchase_id).map_err(|_| CheckoutError::Unavailable)?,
            deadline,
        )
        .map_err(|_| CheckoutError::Unavailable)?;
    for hold in holds {
        let inserted = sqlx::query(
            "INSERT INTO finance_creator_earnings
                (purchase_id,user_id,share_percent,amount_minor,clear_at)
             SELECT $1,$2,s.share_percent,$3,p.refund_deadline_at
             FROM chapter_purchase_share_lines s
             JOIN chapter_purchase_facts p ON p.id=s.purchase_id
             WHERE s.purchase_id=$1 AND s.user_id=$2",
        )
        .bind(purchase_id)
        .bind(hold.recipient_id.get())
        .bind(hold.amount_minor.get())
        .execute(&mut **tx)
        .await
        .map_err(|_| CheckoutError::Unavailable)?;
        if inserted.rows_affected() != 1 {
            return Err(CheckoutError::Unavailable);
        }
    }
    Ok(())
}
