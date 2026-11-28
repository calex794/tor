//! PostgreSQL serialization point for all outbound quota reservations.
use crate::domain::{
    launch::DispatchGrant,
    quota::{QuotaDecision, QuotaKeys},
    MailClaim,
};
use sqlx::PgPool;
use tor_contracts::mail::MailPurpose;

pub(super) mod ledger;

#[derive(Clone)]
pub struct PgMailQuota {
    pool: PgPool,
}

impl PgMailQuota {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Reserve immediately before a single provider submission. Never hold
    /// this transaction while doing DNS/SMTP. On errors or ambiguous commit,
    /// do not send. This is a budget permit, not a delivery or SMTP receipt.
    pub async fn reserve(
        &self,
        claim: MailClaim,
        keys: QuotaKeys,
    ) -> Result<QuotaDecision, sqlx::Error> {
        self.reserve_inner(claim, keys, None).await
    }

    /// Common live dispatch reservation. The exact activation receipt is
    /// persisted on the charged row under the same budget lock/transaction.
    pub async fn reserve_dispatch(
        &self,
        grant: &DispatchGrant,
        keys: QuotaKeys,
    ) -> Result<QuotaDecision, sqlx::Error> {
        self.reserve_inner(grant.claim(), keys, Some(grant)).await
    }

    async fn reserve_inner(
        &self,
        claim: MailClaim,
        keys: QuotaKeys,
        grant: Option<&DispatchGrant>,
    ) -> Result<QuotaDecision, sqlx::Error> {
        if claim.intent.purpose == MailPurpose::RegistrationConfirmation
            && !keys.matches_registration(claim.intent)
        {
            return Ok(QuotaDecision::RegistrationEvidenceRequired);
        }
        if claim.intent.purpose == MailPurpose::PasswordReset
            && !keys.matches_password_reset(claim.intent)
        {
            return Ok(QuotaDecision::PasswordResetEvidenceRequired);
        }
        let mut tx = self.pool.begin().await?;
        // Serialize all providers/credentials/workers against the same initial
        // budget. Do not use transaction-start time after waiting for this lock.
        let key_binding = sqlx::query_scalar::<_, Option<Vec<u8>>>(
            "SELECT key_binding FROM mail_send_budget_guard WHERE singleton FOR UPDATE",
        )
        .fetch_one(&mut *tx)
        .await?;
        let allowed = match grant {
            Some(grant) => super::launch::dispatch_matches(&mut tx, grant, false).await?,
            None => super::launch::legacy_allowed(&mut tx).await?,
        };
        if !allowed {
            tx.rollback().await?;
            return Ok(QuotaDecision::DispatchNotAuthorized);
        }

        let active = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM mail_outbox WHERE id=$1 AND generation=$2 \
             AND purpose=$3 AND event_id=$4 AND recipient_user_id=$5 \
             AND status='leased' AND lease_until>clock_timestamp() FOR UPDATE",
        )
        .bind(claim.id)
        .bind(claim.generation)
        .bind(claim.intent.purpose.as_str())
        .bind(claim.intent.event_id.get())
        .bind(claim.intent.recipient.get())
        .fetch_optional(&mut *tx)
        .await?;
        if active.is_none() {
            tx.rollback().await?;
            return Ok(QuotaDecision::StaleClaim);
        }

        let decision = ledger::reserve_ledger(
            &mut tx,
            key_binding,
            claim.id,
            claim.generation,
            claim.intent.purpose.as_str(),
            keys,
            None,
        )
        .await?;
        if decision == QuotaDecision::Reserved {
            let cost = super::cost::record_reservation(&mut tx, claim).await?;
            if cost != QuotaDecision::Reserved {
                // Neither the short quota ledger nor the monthly counter may
                // commit when admission is denied. No SMTP permit is issued.
                tx.rollback().await?;
                return Ok(cost);
            }
            if let Some(grant) = grant {
                let Some(digest) = grant.digest() else {
                    return Ok(QuotaDecision::DispatchNotAuthorized);
                };
                let bound = sqlx::query(
                    "UPDATE mail_send_reservations SET launch_intent_digest=$3 \
                     WHERE outbox_id=$1 AND generation=$2 AND launch_intent_digest IS NULL",
                )
                .bind(claim.id)
                .bind(claim.generation)
                .bind(digest.as_slice())
                .execute(&mut *tx)
                .await?
                .rows_affected();
                if bound != 1 || !super::launch::dispatch_matches(&mut tx, grant, true).await? {
                    tx.rollback().await?;
                    return Ok(QuotaDecision::DispatchNotAuthorized);
                }
            }
        }
        if decision == QuotaDecision::Reserved {
            tx.commit().await?;
        } else {
            tx.rollback().await?;
        }
        Ok(decision)
    }
}
