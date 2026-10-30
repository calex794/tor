//! D-06: private, not HTTP-mounted, idempotent refund *request* persistence.
//! A pending request is NOT permission to pay a refund. No approval, wallet
//! credit, creator reversal, access revocation or external settlement here.
//! The approved policy catalog starts empty; legacy checkout remains NULL.

use crate::domain::money::IdempotencyKey;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectRefundRequestError {
    InvalidRequest,
    NotFound,
    PolicyUnavailable,
    WindowExpired,
    Revoked,
    UnverifiedPurchase,
    AlreadyOpen,
    DuplicateKeyDifferentRequest,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RefundRequestReceipt {
    pub request_id: i64,
    pub purchase_id: i64,
    pub created: bool,
}

#[derive(Clone)]
pub struct PgDirectRefundRequests {
    pool: PgPool,
}

impl PgDirectRefundRequests {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Claim one buyer-owned purchase in PostgreSQL. The purchase lock
    /// serializes requests across replicas; unique constraints are the
    /// second line of defence. Approval must independently recheck *all*
    /// facts, including first-grant evidence, in its money transaction.
    pub async fn request_once(
        &self,
        actor: i64,
        purchase_id: i64,
        key: &IdempotencyKey,
        reason: &str,
    ) -> Result<RefundRequestReceipt, DirectRefundRequestError> {
        let reason = reason.trim();
        if actor <= 0
            || purchase_id <= 0
            || reason.is_empty()
            || reason.chars().count() > 2000
        {
            return Err(DirectRefundRequestError::InvalidRequest);
        }
        let digest = request_digest(actor, purchase_id, reason);
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| DirectRefundRequestError::Unavailable)?;

        let purchase = sqlx::query(
            "SELECT p.refund_policy_version,
                    COALESCE(p.refund_deadline_at < now(),false) AS expired,
                    EXISTS (
                      SELECT 1 FROM finance_direct_refund_policy_catalog c
                      WHERE c.version=p.refund_policy_version
                        AND c.approved_at<=p.acquired_at
                        AND c.effective_at<=p.acquired_at
                        AND p.refund_deadline_at<=p.acquired_at
                            + (c.window_seconds * INTERVAL '1 second')
                    ) AS approved_policy,
                    EXISTS (
                      SELECT 1 FROM chapter_purchase_revocations r
                      WHERE r.purchase_id=p.id
                    ) AS revoked,
                    (SELECT count(*) FROM reader_wallet_entries e
                     WHERE e.user_id=p.user_id AND e.purchase_id=p.id
                       AND e.kind='chapter_purchase'
                       AND e.delta_minor=-p.amount_minor
                       AND e.reference=p.settlement_reference) AS buyer_debits,
                    EXISTS (
                      SELECT 1 FROM chapter_purchase_share_snapshots s
                      WHERE s.purchase_id=p.id
                        AND s.gross_amount_minor=p.amount_minor
                        AND (SELECT COALESCE(sum(l.share_percent),0)
                             FROM chapter_purchase_share_lines l
                             WHERE l.purchase_id=s.purchase_id)=100
                    ) AS source_shares
             FROM chapter_purchase_facts p
             JOIN library_purchased_chapters l
               ON l.purchase_id=p.id AND l.user_id=p.user_id
              AND l.chapter_id=p.chapter_id AND l.active_version_id=p.version_id
             WHERE p.id=$1 AND p.user_id=$2 FOR UPDATE OF p",
        )
        .bind(purchase_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| DirectRefundRequestError::Unavailable)?
        .ok_or(DirectRefundRequestError::NotFound)?;

        // Replaying a prior request must not depend on today's deadline.
        // A different body with the same key is always a conflict.
        let prior = sqlx::query(
            "SELECT id,purchase_id,request_digest
             FROM finance_direct_refund_requests
             WHERE user_id=$1 AND request_key=$2",
        )
        .bind(actor)
        .bind(key.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| DirectRefundRequestError::Unavailable)?;
        if let Some(prior) = prior {
            let old_purchase: i64 = prior
                .try_get("purchase_id")
                .map_err(|_| DirectRefundRequestError::Unavailable)?;
            let old_digest: Vec<u8> = prior
                .try_get("request_digest")
                .map_err(|_| DirectRefundRequestError::Unavailable)?;
            if old_purchase != purchase_id || old_digest != digest {
                return Err(DirectRefundRequestError::DuplicateKeyDifferentRequest);
            }
            return Ok(RefundRequestReceipt {
                request_id: prior
                    .try_get("id")
                    .map_err(|_| DirectRefundRequestError::Unavailable)?,
                purchase_id,
                created: false,
            });
        }

        let revoked: bool = purchase
            .try_get("revoked")
            .map_err(|_| DirectRefundRequestError::Unavailable)?;
        if revoked {
            return Err(DirectRefundRequestError::Revoked);
        }
        let policy_version: Option<String> = purchase
            .try_get("refund_policy_version")
            .map_err(|_| DirectRefundRequestError::Unavailable)?;
        let approved_policy: bool = purchase
            .try_get("approved_policy")
            .map_err(|_| DirectRefundRequestError::Unavailable)?;
        if policy_version.is_none() || !approved_policy {
            return Err(DirectRefundRequestError::PolicyUnavailable);
        }
        let expired: bool = purchase
            .try_get("expired")
            .map_err(|_| DirectRefundRequestError::Unavailable)?;
        if expired {
            return Err(DirectRefundRequestError::WindowExpired);
        }
        let buyer_debits: i64 = purchase
            .try_get("buyer_debits")
            .map_err(|_| DirectRefundRequestError::Unavailable)?;
        let source_shares: bool = purchase
            .try_get("source_shares")
            .map_err(|_| DirectRefundRequestError::Unavailable)?;
        if buyer_debits != 1 || !source_shares {
            return Err(DirectRefundRequestError::UnverifiedPurchase);
        }
        let already_open: bool = sqlx::query_scalar(
            "SELECT EXISTS(
              SELECT 1 FROM finance_direct_refund_requests
              WHERE purchase_id=$1 AND state IN ('pending','processing')
            )",
        )
        .bind(purchase_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| DirectRefundRequestError::Unavailable)?;
        if already_open {
            return Err(DirectRefundRequestError::AlreadyOpen);
        }

        let request_id: i64 = sqlx::query_scalar(
            "INSERT INTO finance_direct_refund_requests
             (purchase_id,user_id,request_key,request_digest,reason)
             VALUES($1,$2,$3,$4,$5) RETURNING id",
        )
        .bind(purchase_id)
        .bind(actor)
        .bind(key.as_str())
        .bind(digest.as_slice())
        .bind(reason)
        .fetch_one(&mut *tx)
        .await
        .map_err(|error| {
            if error
                .as_database_error()
                .and_then(|db| db.code())
                .as_deref()
                == Some("23505")
            {
                DirectRefundRequestError::AlreadyOpen
            } else {
                DirectRefundRequestError::Unavailable
            }
        })?;
        tx.commit()
            .await
            .map_err(|_| DirectRefundRequestError::Unavailable)?;
        Ok(RefundRequestReceipt {
            request_id,
            purchase_id,
            created: true,
        })
    }
}

fn request_digest(actor: i64, purchase: i64, reason: &str) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"tor.direct.refund.request.v1");
    hash.update(actor.to_be_bytes());
    hash.update(purchase.to_be_bytes());
    hash.update((reason.len() as u64).to_be_bytes());
    hash.update(reason.as_bytes());
    hash.finalize().into()
}
