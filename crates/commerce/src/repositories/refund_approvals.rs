//! D-06: private PostgreSQL-only approval for an unopened direct chapter.
//! This is internal wallet reversal, NOT an external banking settlement.
//! No public route is mounted. Approval must additionally require moderator
//! step-up before HTTP wiring. All money and right changes commit together.

use sqlx::{PgPool, Row};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectRefundApproveError {
    InvalidRequest,
    Forbidden,
    NotFound,
    RevisionConflict,
    Conflict,
    PolicyUnavailable,
    WindowExpired,
    AlreadyOpened,
    Revoked,
    UnverifiedPurchase,
    Overflow,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovedRefundReceipt {
    pub request_id: i64,
    pub purchase_id: i64,
    pub revision: i64,
    pub credited_minor: i64,
    pub replayed: bool,
}

#[derive(Clone)]
pub struct PgDirectRefundApprovals {
    pool: PgPool,
}

impl PgDirectRefundApprovals {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Writer lock order: active moderator -> buyer wallet -> purchase ->
    /// refund request. The purchase's FOR UPDATE lock serializes approval
    /// against C-06's first protected grant and prevents the open/refund race.
    /// Identical successful retries return the same receipt without any new
    /// wallet movement. A different decision, actor or reason is a conflict.
    pub async fn approve_once(
        &self,
        actor: i64,
        request_id: i64,
        expected_revision: i64,
        reason: &str,
    ) -> Result<ApprovedRefundReceipt, DirectRefundApproveError> {
        let reason = reason.trim();
        if actor <= 0
            || request_id <= 0
            || expected_revision <= 0
            || reason.is_empty()
            || reason.chars().count() > 1000
        {
            return Err(DirectRefundApproveError::InvalidRequest);
        }

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| DirectRefundApproveError::Unavailable)?;
        // Authorization is live and held until commit. There is no creator
        // self-refund or non-administrator shortcut.
        let allowed: Option<i64> = sqlx::query_scalar(
            "SELECT r.user_id FROM account_roles r
             JOIN users u ON u.id=r.user_id
             WHERE r.user_id=$1 AND r.role='administrator' AND r.is_active
               AND u.account_state='active'
             FOR SHARE OF r,u",
        )
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| DirectRefundApproveError::Unavailable)?;
        if allowed.is_none() {
            return Err(DirectRefundApproveError::Forbidden);
        }

        let meta = sqlx::query(
            "SELECT purchase_id,user_id
             FROM finance_direct_refund_requests WHERE id=$1",
        )
        .bind(request_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| DirectRefundApproveError::Unavailable)?
        .ok_or(DirectRefundApproveError::NotFound)?;
        let purchase_id: i64 = meta
            .try_get("purchase_id")
            .map_err(|_| DirectRefundApproveError::Unavailable)?;
        let buyer: i64 = meta
            .try_get("user_id")
            .map_err(|_| DirectRefundApproveError::Unavailable)?;

        // Match the checkout lock order. A concurrent checkout must never
        // overwrite the refunded buyer balance or reverse a non-owned fact.
        let balance: i64 = sqlx::query_scalar(
            "SELECT balance_minor FROM reader_wallet_accounts
             WHERE user_id=$1 AND currency='RUB' FOR UPDATE",
        )
        .bind(buyer)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| DirectRefundApproveError::Unavailable)?
        .ok_or(DirectRefundApproveError::UnverifiedPurchase)?;

        let purchase = sqlx::query(
            "SELECT p.amount_minor,p.refund_policy_version,p.refund_deadline_at,
                    p.first_grant_tracking_version,p.acquired_at,p.settlement_reference,
                    fp.platform_fee_minor,fp.creator_pool_minor,
                    fp.refund_policy_version AS finance_refund_policy,
                    fp.refund_deadline_at AS finance_refund_deadline,
                    EXISTS(
                        SELECT 1 FROM finance_direct_refund_policy_catalog policy
                        WHERE policy.version=p.refund_policy_version
                          AND policy.approved_at<=p.acquired_at
                          AND policy.effective_at<=p.acquired_at
                          AND p.refund_deadline_at<=p.acquired_at
                              + (policy.window_seconds * INTERVAL '1 second')
                    ) AS approved_policy
             FROM chapter_purchase_facts p
             JOIN library_purchased_chapters l ON l.purchase_id=p.id
                 AND l.user_id=p.user_id AND l.chapter_id=p.chapter_id
                 AND l.active_version_id=p.version_id
             JOIN finance_chapter_postings fp ON fp.purchase_id=p.id
             WHERE p.id=$1 AND p.user_id=$2 FOR UPDATE OF p",
        )
        .bind(purchase_id)
        .bind(buyer)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| DirectRefundApproveError::Unavailable)?
        .ok_or(DirectRefundApproveError::UnverifiedPurchase)?;

        let decision = sqlx::query(
            "SELECT purchase_id,user_id,state,revision,decision_actor_id,
                    decision_reason,decision_reference
             FROM finance_direct_refund_requests WHERE id=$1 FOR UPDATE",
        )
        .bind(request_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| DirectRefundApproveError::Unavailable)?
        .ok_or(DirectRefundApproveError::NotFound)?;
        let state: String = decision
            .try_get("state")
            .map_err(|_| DirectRefundApproveError::Unavailable)?;
        let revision: i64 = decision
            .try_get("revision")
            .map_err(|_| DirectRefundApproveError::Unavailable)?;

        if state == "approved" {
            let previous_actor: Option<i64> = decision
                .try_get("decision_actor_id")
                .map_err(|_| DirectRefundApproveError::Unavailable)?;
            let previous_reason: Option<String> = decision
                .try_get("decision_reason")
                .map_err(|_| DirectRefundApproveError::Unavailable)?;
            if previous_actor != Some(actor) || previous_reason.as_deref() != Some(reason) {
                return Err(DirectRefundApproveError::Conflict);
            }
            let amount: Option<i64> = sqlx::query_scalar(
                "SELECT amount_minor FROM finance_direct_refund_postings
                 WHERE purchase_id=$1 AND request_id=$2 AND buyer_user_id=$3",
            )
            .bind(purchase_id)
            .bind(request_id)
            .bind(buyer)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| DirectRefundApproveError::Unavailable)?;
            return Ok(ApprovedRefundReceipt {
                request_id,
                purchase_id,
                revision,
                credited_minor: amount.ok_or(DirectRefundApproveError::UnverifiedPurchase)?,
                replayed: true,
            });
        }
        if state != "pending" {
            return Err(DirectRefundApproveError::Conflict);
        }
        if revision != expected_revision {
            return Err(DirectRefundApproveError::RevisionConflict);
        }
        if decision
            .try_get::<i64, _>("purchase_id")
            .map_err(|_| DirectRefundApproveError::Unavailable)?
            != purchase_id
            || decision
                .try_get::<i64, _>("user_id")
                .map_err(|_| DirectRefundApproveError::Unavailable)?
                != buyer
        {
            return Err(DirectRefundApproveError::UnverifiedPurchase);
        }
        let tracking: Option<String> = purchase
            .try_get("first_grant_tracking_version")
            .map_err(|_| DirectRefundApproveError::Unavailable)?;
        let policy: Option<String> = purchase
            .try_get("refund_policy_version")
            .map_err(|_| DirectRefundApproveError::Unavailable)?;
        let approved: bool = purchase
            .try_get("approved_policy")
            .map_err(|_| DirectRefundApproveError::Unavailable)?;
        if policy.is_none() || !approved {
            return Err(DirectRefundApproveError::PolicyUnavailable);
        }
        if tracking.as_deref() != Some("c06-first-issue-v1") {
            return Err(DirectRefundApproveError::UnverifiedPurchase);
        }
        let within_deadline: bool = sqlx::query_scalar(
            "SELECT clock_timestamp() <= refund_deadline_at
             FROM chapter_purchase_facts WHERE id=$1",
        )
        .bind(purchase_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| DirectRefundApproveError::Unavailable)?;
        if !within_deadline {
            return Err(DirectRefundApproveError::WindowExpired);
        }
        // After acquiring the purchase lock, re-read exposed/revoked state
        // from a fresh READ COMMITTED command; a competing grant committed
        // before us is now visible.
        let evidence = sqlx::query(
            "SELECT
                EXISTS(SELECT 1 FROM chapter_purchase_first_exposures
                       WHERE purchase_id=$1) AS opened,
                EXISTS(SELECT 1 FROM chapter_purchase_revocations
                       WHERE purchase_id=$1) AS revoked",
        )
        .bind(purchase_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| DirectRefundApproveError::Unavailable)?;
        if evidence
            .try_get::<bool, _>("revoked")
            .map_err(|_| DirectRefundApproveError::Unavailable)?
        {
            return Err(DirectRefundApproveError::Revoked);
        }
        if evidence
            .try_get::<bool, _>("opened")
            .map_err(|_| DirectRefundApproveError::Unavailable)?
        {
            return Err(DirectRefundApproveError::AlreadyOpened);
        }

        let amount: i64 = purchase
            .try_get("amount_minor")
            .map_err(|_| DirectRefundApproveError::Unavailable)?;
        let platform: i64 = purchase
            .try_get("platform_fee_minor")
            .map_err(|_| DirectRefundApproveError::Unavailable)?;
        let creator: i64 = purchase
            .try_get("creator_pool_minor")
            .map_err(|_| DirectRefundApproveError::Unavailable)?;
        let finance_policy: String = purchase
            .try_get("finance_refund_policy")
            .map_err(|_| DirectRefundApproveError::Unavailable)?;
        let deadline_matches: bool = sqlx::query_scalar(
            "SELECT p.refund_deadline_at=fp.refund_deadline_at
             FROM chapter_purchase_facts p
             JOIN finance_chapter_postings fp ON fp.purchase_id=p.id
             WHERE p.id=$1",
        )
        .bind(purchase_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| DirectRefundApproveError::Unavailable)?;
        if !deadline_matches
            || policy.as_deref() != Some(finance_policy.as_str())
            || amount <= 0
            || platform < 0
            || creator < 0
            || platform.checked_add(creator) != Some(amount)
        {
            return Err(DirectRefundApproveError::UnverifiedPurchase);
        }
        let source_correct: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1
                FROM finance_platform_credits pc
                JOIN finance_chapter_postings fp ON fp.purchase_id=pc.purchase_id
                WHERE fp.purchase_id=$1 AND pc.amount_minor=fp.platform_fee_minor
                  AND (SELECT COALESCE(SUM(ce.amount_minor),0)
                       FROM finance_creator_earnings ce
                       WHERE ce.purchase_id=$1)=fp.creator_pool_minor
                  AND (SELECT COUNT(*) FROM reader_wallet_entries e
                       JOIN chapter_purchase_facts p ON p.id=e.purchase_id
                       WHERE e.purchase_id=$1 AND e.user_id=p.user_id
                         AND e.kind='chapter_purchase'
                         AND e.delta_minor=-p.amount_minor
                         AND e.reference=p.settlement_reference)=1
            )",
        )
        .bind(purchase_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| DirectRefundApproveError::Unavailable)?;
        if !source_correct {
            return Err(DirectRefundApproveError::UnverifiedPurchase);
        }
        let new_balance = balance
            .checked_add(amount)
            .ok_or(DirectRefundApproveError::Overflow)?;
        let reference = format!("d06:refund:{purchase_id}");

        // A pending state is internal to this transaction. Nothing external
        // can see 'processing' unless the entire transaction finishes.
        sqlx::query(
            "UPDATE finance_direct_refund_requests
             SET state='processing',revision=revision+1
             WHERE id=$1 AND state='pending' AND revision=$2",
        )
        .bind(request_id)
        .bind(expected_revision)
        .execute(&mut *tx)
        .await
        .map_err(|_| DirectRefundApproveError::Unavailable)?;

        sqlx::query("UPDATE reader_wallet_accounts SET balance_minor=$2 WHERE user_id=$1")
            .bind(buyer)
            .bind(new_balance)
            .execute(&mut *tx)
            .await
            .map_err(|_| DirectRefundApproveError::Unavailable)?;
        sqlx::query(
            "INSERT INTO reader_wallet_entries
              (user_id,kind,delta_minor,balance_after_minor,reference,purchase_id)
             VALUES($1,'chapter_refund',$2,$3,$4,$5)",
        )
        .bind(buyer)
        .bind(amount)
        .bind(new_balance)
        .bind(&reference)
        .bind(purchase_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| DirectRefundApproveError::Unavailable)?;

        sqlx::query(
            "INSERT INTO chapter_purchase_revocations
              (purchase_id,reversal_reference,reason)
             VALUES($1,$2,'refund')",
        )
        .bind(purchase_id)
        .bind(&reference)
        .execute(&mut *tx)
        .await
        .map_err(|_| DirectRefundApproveError::Unavailable)?;

        sqlx::query(
            "INSERT INTO finance_direct_refund_postings
               (purchase_id,request_id,buyer_user_id,amount_minor,
                platform_reversal_minor,creator_reversal_minor,reversal_reference)
             VALUES($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(purchase_id)
        .bind(request_id)
        .bind(buyer)
        .bind(amount)
        .bind(platform)
        .bind(creator)
        .bind(&reference)
        .execute(&mut *tx)
        .await
        .map_err(|_| DirectRefundApproveError::Unavailable)?;

        sqlx::query(
            "INSERT INTO finance_direct_refund_creator_reversals
               (purchase_id,user_id,amount_minor)
             SELECT purchase_id,user_id,amount_minor
             FROM finance_creator_earnings WHERE purchase_id=$1",
        )
        .bind(purchase_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| DirectRefundApproveError::Unavailable)?;

        let final_revision: i64 = sqlx::query_scalar(
            "UPDATE finance_direct_refund_requests
             SET state='approved',revision=revision+1,
                 decision_actor_id=$2,decision_reason=$3,
                 decision_reference=$4,decided_at=clock_timestamp()
             WHERE id=$1 AND state='processing' AND revision=$5
             RETURNING revision",
        )
        .bind(request_id)
        .bind(actor)
        .bind(reason)
        .bind(&reference)
        .bind(
            expected_revision
                .checked_add(1)
                .ok_or(DirectRefundApproveError::Overflow)?,
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| DirectRefundApproveError::Unavailable)?
        .ok_or(DirectRefundApproveError::RevisionConflict)?;
        tx.commit()
            .await
            .map_err(|_| DirectRefundApproveError::Unavailable)?;
        Ok(ApprovedRefundReceipt {
            request_id,
            purchase_id,
            revision: final_revision,
            credited_minor: amount,
            replayed: false,
        })
    }
}
