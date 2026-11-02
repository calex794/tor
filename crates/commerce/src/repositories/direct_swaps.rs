//! D-06: private, all-or-nothing swap writer. No HTTP route is mounted.
//! Quotes do not authorize posting. The legal policy catalog starts empty.

use crate::domain::money::IdempotencyKey;
use crate::repositories::checkout::revenue;
use crate::repositories::swap_intent::DirectSwapIntent;
use sqlx::{PgPool, Row};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SwapError {
    Invalid,
    Conflict,
    NotFound,
    Ineligible,
    Changed,
    AlreadyOwned,
    Opened,
    Refunding,
    Expired,
    NoPolicy,
    Insufficient,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SwapReceipt {
    pub old_purchase: i64,
    pub new_purchase: i64,
    pub delta_minor: i64,
    pub balance_after: i64,
    pub replayed: bool,
}

#[derive(Clone)]
pub struct PgDirectSwaps {
    pool: PgPool,
}

impl PgDirectSwaps {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Reuses D-08's frozen allocation writer and the purchase-grant trigger.
    /// Lock ordering mirrors checkout: published offer -> wallet -> purchase.
    pub async fn swap_once(
        &self,
        i: DirectSwapIntent,
        key: &IdempotencyKey,
    ) -> Result<SwapReceipt, SwapError> {
        let digest = i.digest().map_err(|_| SwapError::Invalid)?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| SwapError::Unavailable)?;
        let inserted: Option<i64> = sqlx::query_scalar(
            "INSERT INTO finance_direct_swap_commands
              (buyer_user_id,request_key,request_digest,source_purchase_id,
               target_chapter_id,target_version_id,source_amount_minor,target_amount_minor)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
             ON CONFLICT (buyer_user_id,request_key) DO NOTHING
             RETURNING buyer_user_id",
        )
        .bind(i.buyer_id)
        .bind(key.as_str())
        .bind(digest.as_slice())
        .bind(i.source_purchase_id)
        .bind(i.target_chapter_id)
        .bind(i.target_version_id)
        .bind(i.source_price_minor)
        .bind(i.target_price_minor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| SwapError::Unavailable)?;
        let claim = sqlx::query(
            "SELECT request_digest,new_purchase_id,balance_after_minor
             FROM finance_direct_swap_commands
             WHERE buyer_user_id=$1 AND request_key=$2 FOR UPDATE",
        )
        .bind(i.buyer_id)
        .bind(key.as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| SwapError::Unavailable)?;
        let prior_digest: Vec<u8> = claim
            .try_get("request_digest")
            .map_err(|_| SwapError::Unavailable)?;
        if prior_digest != digest {
            return Err(SwapError::Conflict);
        }
        let delta = i
            .target_price_minor
            .checked_sub(i.source_price_minor)
            .ok_or(SwapError::Invalid)?;
        if inserted.is_none() {
            let new_purchase: i64 = claim
                .try_get::<Option<i64>, _>("new_purchase_id")
                .map_err(|_| SwapError::Unavailable)?
                .ok_or(SwapError::Unavailable)?;
            let balance_after: i64 = claim
                .try_get::<Option<i64>, _>("balance_after_minor")
                .map_err(|_| SwapError::Unavailable)?
                .ok_or(SwapError::Unavailable)?;
            let verified: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM finance_direct_swap_postings
                 WHERE source_purchase_id=$1 AND new_purchase_id=$2
                   AND buyer_user_id=$3 AND request_key=$4)",
            )
            .bind(i.source_purchase_id)
            .bind(new_purchase)
            .bind(i.buyer_id)
            .bind(key.as_str())
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| SwapError::Unavailable)?;
            if !verified {
                return Err(SwapError::Unavailable);
            }
            tx.commit().await.map_err(|_| SwapError::Unavailable)?;
            return Ok(SwapReceipt {
                old_purchase: i.source_purchase_id,
                new_purchase,
                delta_minor: delta,
                balance_after,
                replayed: true,
            });
        }

        let old_book: i64 = sqlx::query_scalar(
            "SELECT c.book_id FROM chapter_purchase_facts p
             JOIN chapters c ON c.id=p.chapter_id
             WHERE p.id=$1 AND p.user_id=$2",
        )
        .bind(i.source_purchase_id)
        .bind(i.buyer_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| SwapError::Unavailable)?
        .ok_or(SwapError::NotFound)?;
        let target_book: i64 = sqlx::query_scalar("SELECT book_id FROM chapters WHERE id=$1")
            .bind(i.target_chapter_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| SwapError::Unavailable)?
            .ok_or(SwapError::Ineligible)?;
        if old_book == target_book {
            return Err(SwapError::Ineligible);
        }
        // Stable book lock order prevents a simultaneous change of owner.
        let books = sqlx::query(
            "SELECT id,owner_user_id FROM books
             WHERE id IN ($1,$2) ORDER BY id FOR SHARE",
        )
        .bind(old_book)
        .bind(target_book)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| SwapError::Unavailable)?;
        if books.len() != 2 {
            return Err(SwapError::Ineligible);
        }
        let owner_a: Option<i64> = books[0]
            .try_get("owner_user_id")
            .map_err(|_| SwapError::Unavailable)?;
        let owner_b: Option<i64> = books[1]
            .try_get("owner_user_id")
            .map_err(|_| SwapError::Unavailable)?;
        if owner_a.is_none() || owner_a != owner_b || owner_a == Some(i.buyer_id) {
            return Err(SwapError::Ineligible);
        }
        let offer = sqlx::query(
            "SELECT c.price_amount,p.version_id,s.current_revision
             FROM chapters c JOIN books b ON b.id=c.book_id
             JOIN chapter_publications p ON p.chapter_id=c.id
             JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=p.version_id
             JOIN revenue_share_states s ON s.book_id=b.id
             WHERE c.id=$1 AND c.book_id=$2 AND c.access_level='paid'
               AND c.visibility='public' AND c.price_amount>0
               AND NOT c.admin_hold AND NOT c.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
               AND b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
             FOR SHARE OF c,p,b",
        )
        .bind(i.target_chapter_id)
        .bind(target_book)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| SwapError::Unavailable)?
        .ok_or(SwapError::Ineligible)?;
        if offer
            .try_get::<i64, _>("price_amount")
            .map_err(|_| SwapError::Unavailable)?
            != i.target_price_minor
            || offer
                .try_get::<i64, _>("version_id")
                .map_err(|_| SwapError::Unavailable)?
                != i.target_version_id
        {
            return Err(SwapError::Changed);
        }
        let revision: i64 = offer
            .try_get("current_revision")
            .map_err(|_| SwapError::Unavailable)?;
        let shares: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(share_percent),0)::BIGINT
             FROM revenue_share_assignments WHERE book_id=$1 AND revision=$2",
        )
        .bind(target_book)
        .bind(revision)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| SwapError::Unavailable)?;
        if shares != 100 {
            return Err(SwapError::Ineligible);
        }
        let terms = revenue::load_terms(&mut tx, target_book, revision, i.target_price_minor)
            .await
            .map_err(|_| SwapError::NoPolicy)?;
        let approved: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM finance_direct_refund_policy_catalog
             WHERE version=$1 AND approved_at<=transaction_timestamp()
               AND effective_at<=transaction_timestamp() AND window_seconds=$2)",
        )
        .bind(&terms.refund_policy_version)
        .bind(terms.refund_window_seconds)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| SwapError::Unavailable)?;
        if !approved {
            return Err(SwapError::NoPolicy);
        }

        let balance: i64 = sqlx::query_scalar(
            "SELECT balance_minor FROM reader_wallet_accounts
             WHERE user_id=$1 AND currency='RUB' FOR UPDATE",
        )
        .bind(i.buyer_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| SwapError::Unavailable)?
        .ok_or(SwapError::NotFound)?;
        let source = sqlx::query(
            "SELECT p.amount_minor,p.refund_policy_version,p.first_grant_tracking_version,
                    fp.platform_fee_minor,fp.creator_pool_minor,
                    (fp.refund_policy_version=p.refund_policy_version
                     AND fp.refund_deadline_at=p.refund_deadline_at
                     AND fp.gross_minor=p.amount_minor
                     AND fp.platform_fee_minor+fp.creator_pool_minor=p.amount_minor) AS matched,
                    EXISTS(SELECT 1 FROM finance_direct_refund_policy_catalog pol
                           WHERE pol.version=p.refund_policy_version
                             AND pol.approved_at<=p.acquired_at
                             AND pol.effective_at<=p.acquired_at
                             AND p.refund_deadline_at<=p.acquired_at+
                                 (pol.window_seconds*INTERVAL '1 second')) AS approved
             FROM chapter_purchase_facts p
             JOIN chapters c ON c.id=p.chapter_id
             JOIN library_purchased_chapters l
               ON l.purchase_id=p.id AND l.user_id=p.user_id
                  AND l.chapter_id=p.chapter_id AND l.active_version_id=p.version_id
             JOIN finance_chapter_postings fp ON fp.purchase_id=p.id
             WHERE p.id=$1 AND p.user_id=$2 AND c.book_id=$3 FOR UPDATE OF p",
        )
        .bind(i.source_purchase_id)
        .bind(i.buyer_id)
        .bind(old_book)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| SwapError::Unavailable)?
        .ok_or(SwapError::NotFound)?;
        if source
            .try_get::<i64, _>("amount_minor")
            .map_err(|_| SwapError::Unavailable)?
            != i.source_price_minor
        {
            return Err(SwapError::Changed);
        }
        if source
            .try_get::<Option<String>, _>("first_grant_tracking_version")
            .map_err(|_| SwapError::Unavailable)?
            .as_deref()
            != Some("c06-first-issue-v1")
            || !source
                .try_get::<bool, _>("matched")
                .map_err(|_| SwapError::Unavailable)?
        {
            return Err(SwapError::Ineligible);
        }
        if !source
            .try_get::<bool, _>("approved")
            .map_err(|_| SwapError::Unavailable)?
        {
            return Err(SwapError::NoPolicy);
        }
        let eligible_time: bool = sqlx::query_scalar(
            "SELECT clock_timestamp()<=refund_deadline_at
             FROM chapter_purchase_facts WHERE id=$1",
        )
        .bind(i.source_purchase_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| SwapError::Unavailable)?;
        if !eligible_time {
            return Err(SwapError::Expired);
        }
        // Separate statement AFTER the purchase lock: exposes a competing
        // first protected read or refund committed while this writer waited.
        let evidence = sqlx::query(
            "SELECT
              EXISTS(SELECT 1 FROM chapter_purchase_first_exposures
                     WHERE purchase_id=$1) AS opened,
              EXISTS(SELECT 1 FROM chapter_purchase_revocations
                     WHERE purchase_id=$1) AS revoked,
              EXISTS(SELECT 1 FROM finance_direct_refund_requests
                     WHERE purchase_id=$1 AND state IN ('pending','processing')) AS refunding,
              EXISTS(SELECT 1 FROM library_purchased_chapters
                     WHERE user_id=$2 AND chapter_id=$3) AS owned,
              (SELECT COUNT(*) FROM reader_wallet_entries e
               JOIN chapter_purchase_facts p ON p.id=e.purchase_id
               WHERE e.purchase_id=$1 AND e.user_id=$2
                 AND e.kind='chapter_purchase' AND e.delta_minor=-p.amount_minor
                 AND e.reference=p.settlement_reference) AS debits,
              (SELECT COALESCE(SUM(share_percent),0)::BIGINT
               FROM chapter_purchase_share_lines WHERE purchase_id=$1) AS shares,
              (SELECT COALESCE(SUM(amount_minor),0)::BIGINT
               FROM finance_creator_earnings WHERE purchase_id=$1) AS creators,
              (SELECT amount_minor FROM finance_platform_credits
               WHERE purchase_id=$1) AS platform",
        )
        .bind(i.source_purchase_id)
        .bind(i.buyer_id)
        .bind(i.target_chapter_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| SwapError::Unavailable)?;
        if evidence
            .try_get::<bool, _>("opened")
            .map_err(|_| SwapError::Unavailable)?
        {
            return Err(SwapError::Opened);
        }
        if evidence
            .try_get::<bool, _>("revoked")
            .map_err(|_| SwapError::Unavailable)?
        {
            return Err(SwapError::Ineligible);
        }
        if evidence
            .try_get::<bool, _>("refunding")
            .map_err(|_| SwapError::Unavailable)?
        {
            return Err(SwapError::Refunding);
        }
        if evidence
            .try_get::<bool, _>("owned")
            .map_err(|_| SwapError::Unavailable)?
        {
            return Err(SwapError::AlreadyOwned);
        }
        let platform: i64 = source
            .try_get("platform_fee_minor")
            .map_err(|_| SwapError::Unavailable)?;
        let creators: i64 = source
            .try_get("creator_pool_minor")
            .map_err(|_| SwapError::Unavailable)?;
        if evidence
            .try_get::<i64, _>("debits")
            .map_err(|_| SwapError::Unavailable)?
            != 1
            || evidence
                .try_get::<i64, _>("shares")
                .map_err(|_| SwapError::Unavailable)?
                != 100
            || evidence
                .try_get::<i64, _>("creators")
                .map_err(|_| SwapError::Unavailable)?
                != creators
            || evidence
                .try_get::<Option<i64>, _>("platform")
                .map_err(|_| SwapError::Unavailable)?
                != Some(platform)
        {
            return Err(SwapError::Ineligible);
        }
        if delta > balance {
            return Err(SwapError::Insufficient);
        }
        let credited = balance
            .checked_add(i.source_price_minor)
            .ok_or(SwapError::Invalid)?;
        let after = credited
            .checked_sub(i.target_price_minor)
            .ok_or(SwapError::Insufficient)?;
        let reverse = format!("d06:swap:old:{}:{}", i.source_purchase_id, key.as_str());
        let settlement = format!("d06:swap:buy:{}:{}", i.buyer_id, key.as_str());
        sqlx::query("UPDATE reader_wallet_accounts SET balance_minor=$2 WHERE user_id=$1")
            .bind(i.buyer_id)
            .bind(credited)
            .execute(&mut *tx)
            .await
            .map_err(|_| SwapError::Unavailable)?;
        sqlx::query(
            "INSERT INTO reader_wallet_entries
              (user_id,kind,delta_minor,balance_after_minor,reference,purchase_id)
             VALUES($1,'chapter_swap_credit',$2,$3,$4,$5)",
        )
        .bind(i.buyer_id)
        .bind(i.source_price_minor)
        .bind(credited)
        .bind(&reverse)
        .bind(i.source_purchase_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| SwapError::Unavailable)?;
        sqlx::query(
            "INSERT INTO chapter_purchase_revocations(purchase_id,reversal_reference,reason)
             VALUES($1,$2,'purchase_swap')",
        )
        .bind(i.source_purchase_id)
        .bind(&reverse)
        .execute(&mut *tx)
        .await
        .map_err(|_| SwapError::Unavailable)?;
        sqlx::query("UPDATE reader_wallet_accounts SET balance_minor=$2 WHERE user_id=$1")
            .bind(i.buyer_id)
            .bind(after)
            .execute(&mut *tx)
            .await
            .map_err(|_| SwapError::Unavailable)?;
        let new_purchase: i64 = sqlx::query_scalar(
            "INSERT INTO chapter_purchase_facts
              (user_id,chapter_id,version_id,settlement_reference,amount_minor,
               currency,first_grant_tracking_version,refund_policy_version,refund_deadline_at)
             VALUES($1,$2,$3,$4,$5,'RUB','c06-first-issue-v1',$6,
                    transaction_timestamp()+($7::BIGINT*INTERVAL '1 second'))
             RETURNING id",
        )
        .bind(i.buyer_id)
        .bind(i.target_chapter_id)
        .bind(i.target_version_id)
        .bind(&settlement)
        .bind(i.target_price_minor)
        .bind(&terms.refund_policy_version)
        .bind(terms.refund_window_seconds)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| SwapError::Unavailable)?;
        sqlx::query(
            "INSERT INTO chapter_purchase_share_snapshots
             (purchase_id,book_id,share_revision,gross_amount_minor) VALUES($1,$2,$3,$4)",
        )
        .bind(new_purchase)
        .bind(target_book)
        .bind(revision)
        .bind(i.target_price_minor)
        .execute(&mut *tx)
        .await
        .map_err(|_| SwapError::Unavailable)?;
        sqlx::query(
            "INSERT INTO chapter_purchase_share_lines(purchase_id,user_id,share_percent)
             SELECT $1,user_id,share_percent FROM revenue_share_assignments
             WHERE book_id=$2 AND revision=$3",
        )
        .bind(new_purchase)
        .bind(target_book)
        .bind(revision)
        .execute(&mut *tx)
        .await
        .map_err(|_| SwapError::Unavailable)?;
        sqlx::query(
            "INSERT INTO reader_wallet_entries
              (user_id,kind,delta_minor,balance_after_minor,reference,purchase_id)
             VALUES($1,'chapter_purchase',$2,$3,$4,$5)",
        )
        .bind(i.buyer_id)
        .bind(-i.target_price_minor)
        .bind(after)
        .bind(&settlement)
        .bind(new_purchase)
        .execute(&mut *tx)
        .await
        .map_err(|_| SwapError::Unavailable)?;
        revenue::post_purchase(&mut tx, new_purchase, target_book, revision, &terms)
            .await
            .map_err(|_| SwapError::Unavailable)?;
        sqlx::query(
            "INSERT INTO finance_direct_swap_postings
              (source_purchase_id,new_purchase_id,buyer_user_id,request_key,
               source_amount_minor,target_amount_minor,platform_reversal_minor,
               creator_reversal_minor,reversal_reference)
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)",
        )
        .bind(i.source_purchase_id)
        .bind(new_purchase)
        .bind(i.buyer_id)
        .bind(key.as_str())
        .bind(i.source_price_minor)
        .bind(i.target_price_minor)
        .bind(platform)
        .bind(creators)
        .bind(&reverse)
        .execute(&mut *tx)
        .await
        .map_err(|_| SwapError::Unavailable)?;
        sqlx::query(
            "INSERT INTO finance_direct_swap_creator_reversals
             (source_purchase_id,user_id,amount_minor)
             SELECT purchase_id,user_id,amount_minor FROM finance_creator_earnings
             WHERE purchase_id=$1",
        )
        .bind(i.source_purchase_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| SwapError::Unavailable)?;
        sqlx::query(
            "UPDATE finance_direct_swap_commands
             SET new_purchase_id=$3,balance_after_minor=$4
             WHERE buyer_user_id=$1 AND request_key=$2",
        )
        .bind(i.buyer_id)
        .bind(key.as_str())
        .bind(new_purchase)
        .bind(after)
        .execute(&mut *tx)
        .await
        .map_err(|_| SwapError::Unavailable)?;
        tx.commit().await.map_err(|_| SwapError::Unavailable)?;
        Ok(SwapReceipt {
            old_purchase: i.source_purchase_id,
            new_purchase,
            delta_minor: delta,
            balance_after: after,
            replayed: false,
        })
    }
}
