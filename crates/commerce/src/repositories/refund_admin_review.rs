//! S036: read-only administrator review of refund requests.
//! An active site administrator is verified from the opaque A session in the
//! same transaction that persists the immutable read audit. No refunds or
//! entitlement changes are possible through this repository.
use sqlx::{PgPool, Row};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdminRefundRequest {
    pub id: i64,
    pub purchase_id: i64,
    pub buyer_user_id: i64,
    pub buyer_name: String,
    pub book_title: String,
    pub chapter_title: String,
    pub amount_minor: i64,
    pub reason: String,
    pub state: String,
    pub revision: i64,
    pub policy_version: Option<String>,
    pub deadline_utc: Option<String>,
    pub created_utc: String,
    pub first_exposed: bool,
    pub revoked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefundAdminViewError {
    InvalidCursor,
    NotFound,
    Unavailable,
}

#[derive(Clone)]
pub struct PgRefundAdminReview {
    pool: PgPool,
}

impl PgRefundAdminReview {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Uses a DB-verified live session and administrator role, never an actor
    /// supplied by the browser. The role and session stay locked until the
    /// audit INSERT and COMMIT succeed. A failed audit yields no read result.
    pub async fn recent(
        &self,
        session_digest: &[u8; 32],
        before: Option<i64>,
    ) -> Result<Vec<AdminRefundRequest>, RefundAdminViewError> {
        if before.is_some_and(|id| id <= 0) {
            return Err(RefundAdminViewError::InvalidCursor);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| RefundAdminViewError::Unavailable)?;
        let administrator: Option<i64> = sqlx::query_scalar(
            "SELECT u.id FROM sessions s
             JOIN users u ON u.id=s.user_id
             JOIN account_roles a ON a.user_id=u.id
             WHERE s.token_hash=$1 AND s.revoked_at IS NULL
               AND s.expires_at>clock_timestamp()
               AND u.account_state='active'
               AND a.role='administrator' AND a.is_active
             FOR SHARE OF s,u,a",
        )
        .bind(session_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| RefundAdminViewError::Unavailable)?;
        let administrator = administrator.ok_or(RefundAdminViewError::NotFound)?;

        // Exactly 30 newest rows; never export the user's email or auth state.
        // These flags are observations, NOT an approval eligibility decision.
        let rows = sqlx::query(
            "SELECT q.id,q.purchase_id,q.user_id AS buyer_user_id,
                    u.display_name AS buyer_name,b.title AS book_title,
                    c.title AS chapter_title,p.amount_minor,
                    q.reason,q.state,q.revision,p.refund_policy_version,
                    to_char(p.refund_deadline_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI')
                        AS deadline_utc,
                    to_char(q.created_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI')
                        AS created_utc,
                    EXISTS(SELECT 1 FROM chapter_purchase_first_exposures e
                           WHERE e.purchase_id=q.purchase_id) AS first_exposed,
                    EXISTS(SELECT 1 FROM chapter_purchase_revocations v
                           WHERE v.purchase_id=q.purchase_id) AS revoked
             FROM finance_direct_refund_requests q
             JOIN chapter_purchase_facts p ON p.id=q.purchase_id AND p.user_id=q.user_id
             JOIN users u ON u.id=q.user_id
             JOIN chapters c ON c.id=p.chapter_id
             JOIN books b ON b.id=c.book_id
             WHERE ($1::bigint IS NULL OR q.id<$1)
             ORDER BY q.id DESC LIMIT 30",
        )
        .bind(before)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| RefundAdminViewError::Unavailable)?;

        let requests: Vec<AdminRefundRequest> = rows
            .into_iter()
            .map(|row| {
                Ok(AdminRefundRequest {
                    id: row
                        .try_get("id")
                        .map_err(|_| RefundAdminViewError::Unavailable)?,
                    purchase_id: row
                        .try_get("purchase_id")
                        .map_err(|_| RefundAdminViewError::Unavailable)?,
                    buyer_user_id: row
                        .try_get("buyer_user_id")
                        .map_err(|_| RefundAdminViewError::Unavailable)?,
                    buyer_name: row
                        .try_get("buyer_name")
                        .map_err(|_| RefundAdminViewError::Unavailable)?,
                    book_title: row
                        .try_get("book_title")
                        .map_err(|_| RefundAdminViewError::Unavailable)?,
                    chapter_title: row
                        .try_get("chapter_title")
                        .map_err(|_| RefundAdminViewError::Unavailable)?,
                    amount_minor: row
                        .try_get("amount_minor")
                        .map_err(|_| RefundAdminViewError::Unavailable)?,
                    reason: row
                        .try_get("reason")
                        .map_err(|_| RefundAdminViewError::Unavailable)?,
                    state: row
                        .try_get("state")
                        .map_err(|_| RefundAdminViewError::Unavailable)?,
                    revision: row
                        .try_get("revision")
                        .map_err(|_| RefundAdminViewError::Unavailable)?,
                    policy_version: row
                        .try_get("refund_policy_version")
                        .map_err(|_| RefundAdminViewError::Unavailable)?,
                    deadline_utc: row
                        .try_get("deadline_utc")
                        .map_err(|_| RefundAdminViewError::Unavailable)?,
                    created_utc: row
                        .try_get("created_utc")
                        .map_err(|_| RefundAdminViewError::Unavailable)?,
                    first_exposed: row
                        .try_get("first_exposed")
                        .map_err(|_| RefundAdminViewError::Unavailable)?,
                    revoked: row
                        .try_get("revoked")
                        .map_err(|_| RefundAdminViewError::Unavailable)?,
                })
            })
            .collect::<Result<_, _>>()?;

        sqlx::query(
            "INSERT INTO finance_direct_refund_admin_access_audit
             (admin_user_id,action,before_id,result_count)
             VALUES($1,'list_refunds',$2,$3)",
        )
        .bind(administrator)
        .bind(before)
        .bind(i32::try_from(requests.len()).map_err(|_| RefundAdminViewError::Unavailable)?)
        .execute(&mut *tx)
        .await
        .map_err(|_| RefundAdminViewError::Unavailable)?;
        tx.commit()
            .await
            .map_err(|_| RefundAdminViewError::Unavailable)?;
        Ok(requests)
    }
}
