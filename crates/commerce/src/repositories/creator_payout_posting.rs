//! S039/MONEY-03: trusted INTERNAL wallet payout posting, no bank transfer.
//! No HTTP exposure until operator security and reconciliation are accepted.
#![forbid(unsafe_code)]

mod credits;

use crate::repositories::creator_finance::PgCreatorFinance;
use sha2::{Digest, Sha256};
use sqlx::PgPool;

const PROOF_SCOPE: &str = "finance.payout.post.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayoutPostError {
    Invalid,
    Forbidden,
    NotFound,
    Conflict,
    StepUpRequired,
    Insufficient,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayoutPostReceipt {
    pub request_id: i64,
    pub recipient_user_id: i64,
    pub amount_minor: i64,
    pub wallet_entry_id: i64,
    pub replayed: bool,
}

#[derive(Clone)]
pub struct PgCreatorPayoutPosting {
    pool: PgPool,
}

fn valid_token(token: &str) -> bool {
    token.len() == 43
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// The trusted proof is bound to the exact immutable request amount and
/// normalized decision reason, not the advisory result or form rendering.
pub fn payout_step_up_payload_digest(
    actor: i64,
    request_id: i64,
    amount_minor: i64,
    reason: &str,
) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(PROOF_SCOPE.as_bytes());
    h.update(actor.to_be_bytes());
    h.update(request_id.to_be_bytes());
    h.update(amount_minor.to_be_bytes());
    h.update((reason.len() as u64).to_be_bytes());
    h.update(reason.as_bytes());
    h.finalize().into()
}

impl PgCreatorPayoutPosting {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Session+role, recipient wallet, recipient user, payout request.
    ///
    /// The receiver's wallet is locked BEFORE the recipient user, matching
    /// checkout's wallet -> reservation-guard order. The owner user lock is
    /// FOR NO KEY UPDATE, compatible with FK KEY SHARE from purchase commands.
    /// A successful retry reads the exact posting, never debits a second time.
    /// Internal 'posted' is an obligation, NOT an external payment confirmation.
    pub async fn approve_once(
        &self,
        actor: i64,
        session_digest: &[u8; 32],
        step_up_token: &str,
        request_id: i64,
        expected_amount_minor: i64,
        reason: &str,
    ) -> Result<PayoutPostReceipt, PayoutPostError> {
        let reason = reason.trim();
        if actor <= 0
            || request_id <= 0
            || !(1..=1_000_000_000_000).contains(&expected_amount_minor)
            || reason.is_empty()
            || reason.chars().count() > 500
            || !valid_token(step_up_token)
        {
            return Err(PayoutPostError::Invalid);
        }
        let payload =
            payout_step_up_payload_digest(actor, request_id, expected_amount_minor, reason);
        let proof_hash = Sha256::digest(step_up_token.as_bytes());
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| PayoutPostError::Unavailable)?;

        let outcome: Result<PayoutPostReceipt, PayoutPostError> = async {
            // Lock current administrator authority until commit (not a stale
            // cached claim). Issuer uses the same role -> session lock order.
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
            .map_err(|_| PayoutPostError::Unavailable)?;
            if allowed.is_none() {
                return Err(PayoutPostError::Forbidden);
            }
            let live: Option<i64> = sqlx::query_scalar(
                "SELECT user_id FROM sessions
                 WHERE user_id=$1 AND token_hash=$2
                   AND revoked_at IS NULL AND expires_at>clock_timestamp()
                 FOR UPDATE",
            )
            .bind(actor)
            .bind(session_digest.as_slice())
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| PayoutPostError::Unavailable)?;
            if live.is_none() {
                return Err(PayoutPostError::StepUpRequired);
            }

            let recipient: Option<i64> =
                sqlx::query_scalar("SELECT user_id FROM finance_creator_payout_requests WHERE id=$1")
                    .bind(request_id)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(|_| PayoutPostError::Unavailable)?;
            let recipient = recipient.ok_or(PayoutPostError::NotFound)?;
            // Independent authorization: administrators cannot post their own
            // payout. Avoids upgrading a held admin SHARE user lock as well.
            if recipient == actor {
                return Err(PayoutPostError::Forbidden);
            }

            sqlx::query(
                "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
                 VALUES($1,0,'RUB') ON CONFLICT(user_id) DO NOTHING",
            )
            .bind(recipient)
            .execute(&mut *tx)
            .await
            .map_err(|_| PayoutPostError::Unavailable)?;
            let balance: i64 = sqlx::query_scalar(
                "SELECT balance_minor FROM reader_wallet_accounts
                 WHERE user_id=$1 AND currency='RUB' FOR UPDATE",
            )
            .bind(recipient)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| PayoutPostError::Unavailable)?;
            let owner: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM users
                 WHERE id=$1 AND account_state='active' FOR NO KEY UPDATE",
            )
            .bind(recipient)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| PayoutPostError::Unavailable)?;
            if owner.is_none() {
                return Err(PayoutPostError::Conflict);
            }

            let request: Option<(i64, String, Option<i64>, Option<String>)> = sqlx::query_as(
                "SELECT amount_minor,state,decision_actor_id,decision_reason
                 FROM finance_creator_payout_requests
                 WHERE id=$1 AND user_id=$2 FOR UPDATE",
            )
            .bind(request_id)
            .bind(recipient)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| PayoutPostError::Unavailable)?;
            let (amount, state, previous_actor, previous_reason) =
                request.ok_or(PayoutPostError::NotFound)?;
            if amount != expected_amount_minor {
                return Err(PayoutPostError::Conflict);
            }
            if state == "posted" {
                if previous_actor != Some(actor) || previous_reason.as_deref() != Some(reason) {
                    return Err(PayoutPostError::Conflict);
                }
                let posting: Option<(i64, i64)> = sqlx::query_as(
                    "SELECT p.wallet_entry_id,p.amount_minor
                     FROM finance_creator_payout_postings p
                     JOIN reader_wallet_entries j ON j.id=p.wallet_entry_id
                     WHERE p.payout_request_id=$1 AND p.recipient_user_id=$2
                       AND p.admin_user_id=$3 AND p.amount_minor=$4
                       AND p.posting_reference=$5 AND j.user_id=$2
                       AND j.kind='creator_payout_debit' AND j.delta_minor=-$4
                       AND j.reference=p.posting_reference",
                )
                .bind(request_id)
                .bind(recipient)
                .bind(actor)
                .bind(amount)
                .bind(format!("s039:payout:{request_id}"))
                .fetch_optional(&mut *tx)
                .await
                .map_err(|_| PayoutPostError::Unavailable)?;
                let (entry, _) = posting.ok_or(PayoutPostError::Unavailable)?;
                return Ok(PayoutPostReceipt {
                    request_id,
                    recipient_user_id: recipient,
                    amount_minor: amount,
                    wallet_entry_id: entry,
                    replayed: true,
                });
            }
            if state != "pending" {
                return Err(PayoutPostError::Conflict);
            }

            // Positive advisory capacity is not sufficient: some matured creator
            // earnings may exist only in the projection, not the spendable wallet.
            // Credit bounded verified sources IN THIS SAME TRANSACTION.
            let actual_balance =
                credits::materialize_until(&mut tx, recipient, amount, balance).await?;
            let totals = PgCreatorFinance::totals_in(&mut tx, recipient)
                .await
                .map_err(|_| PayoutPostError::Unavailable)?;
            let other_reserved = totals
                .reserved_minor
                .checked_sub(amount)
                .filter(|value| *value >= 0)
                .ok_or(PayoutPostError::Unavailable)?;
            let signed = actual_balance
                .checked_add(totals.mature_minor)
                .and_then(|v| v.checked_sub(totals.open_refund_hold_minor))
                .and_then(|v| v.checked_sub(other_reserved))
                .ok_or(PayoutPostError::Unavailable)?;
            if signed < amount || actual_balance < amount {
                return Err(PayoutPostError::Insufficient);
            }

            // Consumption is durable only if the linked debit and posting commit.
            let valid: Option<bool> = sqlx::query_scalar(
                "SELECT COALESCE(
                    step_up_proof_hash=$1 AND step_up_scope=$2
                    AND step_up_resource_id=$3 AND step_up_revision=$4
                    AND step_up_payload_digest=$5
                    AND step_up_issued_at<=clock_timestamp()
                    AND step_up_expires_at>clock_timestamp()
                    AND step_up_used_at IS NULL,
                    FALSE
                 ) FROM sessions WHERE token_hash=$6 AND user_id=$7",
            )
            .bind(proof_hash.as_slice())
            .bind(PROOF_SCOPE)
            .bind(request_id)
            .bind(amount)
            .bind(payload.as_slice())
            .bind(session_digest.as_slice())
            .bind(actor)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| PayoutPostError::Unavailable)?;
            if !valid.unwrap_or(false) {
                return Err(PayoutPostError::StepUpRequired);
            }
            let consumed = sqlx::query(
                "UPDATE sessions SET step_up_used_at=clock_timestamp()
                 WHERE token_hash=$1 AND user_id=$2
                   AND step_up_proof_hash=$3 AND step_up_scope=$4
                   AND step_up_used_at IS NULL AND step_up_expires_at>clock_timestamp()",
            )
            .bind(session_digest.as_slice())
            .bind(actor)
            .bind(proof_hash.as_slice())
            .bind(PROOF_SCOPE)
            .execute(&mut *tx)
            .await
            .map_err(|_| PayoutPostError::Unavailable)?;
            if consumed.rows_affected() != 1 {
                return Err(PayoutPostError::StepUpRequired);
            }

            // Transition first, so S038's wallet guard excludes this ONE pending
            // reservation during the debit; it still protects all other holds.
            let updated = sqlx::query(
                "UPDATE finance_creator_payout_requests
                 SET state='posted',decided_at=clock_timestamp(),
                     decision_actor_id=$2,decision_reason=$3
                 WHERE id=$1 AND state='pending'",
            )
            .bind(request_id)
            .bind(actor)
            .bind(reason)
            .execute(&mut *tx)
            .await
            .map_err(|_| PayoutPostError::Unavailable)?;
            if updated.rows_affected() != 1 {
                return Err(PayoutPostError::Conflict);
            }
            let new_balance: Option<i64> = sqlx::query_scalar(
                "UPDATE reader_wallet_accounts
                 SET balance_minor=balance_minor-$2
                 WHERE user_id=$1 AND currency='RUB' AND balance_minor>=$2
                 RETURNING balance_minor",
            )
            .bind(recipient)
            .bind(amount)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| PayoutPostError::Unavailable)?;
            let new_balance = new_balance.ok_or(PayoutPostError::Insufficient)?;
            let reference = format!("s039:payout:{request_id}");
            let entry_id: i64 = sqlx::query_scalar(
                "INSERT INTO reader_wallet_entries
                 (user_id,kind,delta_minor,balance_after_minor,reference)
                 VALUES($1,'creator_payout_debit',$2,$3,$4) RETURNING id",
            )
            .bind(recipient)
            .bind(-amount)
            .bind(new_balance)
            .bind(&reference)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| PayoutPostError::Unavailable)?;
            sqlx::query(
                "INSERT INTO finance_creator_payout_postings
                 (payout_request_id,recipient_user_id,admin_user_id,
                  amount_minor,wallet_entry_id,posting_reference)
                 VALUES($1,$2,$3,$4,$5,$6)",
            )
            .bind(request_id)
            .bind(recipient)
            .bind(actor)
            .bind(amount)
            .bind(entry_id)
            .bind(&reference)
            .execute(&mut *tx)
            .await
            .map_err(|_| PayoutPostError::Unavailable)?;
            Ok(PayoutPostReceipt {
                request_id,
                recipient_user_id: recipient,
                amount_minor: amount,
                wallet_entry_id: entry_id,
                replayed: false,
            })
        }.await;
        // A rejected or failed posting must release all retained role, session
        // and wallet locks before the caller can revoke authority or retry.
        match outcome {
            Ok(receipt) => {
                tx.commit()
                    .await
                    .map_err(|_| PayoutPostError::Unavailable)?;
                Ok(receipt)
            }
            Err(error) => {
                tx.rollback()
                    .await
                    .map_err(|_| PayoutPostError::Unavailable)?;
                Err(error)
            }
        }
    }
}
