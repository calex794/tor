//! D-10: one Boosty charge = at most one atomic wallet/journal credit.
//! Only linked, server-verified payers are eligible. Provider I/O is forbidden
//! in this repository and no client-supplied evidence can reach it by HTTP.
use crate::domain::boosty_credit::{
    BoostyCreditError, BoostyCreditReceipt, BoostyCreditRepository, VerifiedSubscriptionCharge,
};
use async_trait::async_trait;
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgBoostyCredits {
    pool: PgPool,
}

impl PgBoostyCredits {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn unavailable(_: sqlx::Error) -> BoostyCreditError {
    BoostyCreditError::Unavailable
}

#[async_trait]
impl BoostyCreditRepository for PgBoostyCredits {
    async fn credit_once(
        &self,
        charge: &VerifiedSubscriptionCharge,
    ) -> Result<BoostyCreditReceipt, BoostyCreditError> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;

        // Only a previously verified Boosty identity may receive a credit.
        // The row lock also prevents unlink/rebind during this transaction.
        let user_id = sqlx::query_scalar::<_, i64>(
            "SELECT user_id FROM boosty_verified_wallet_links
             WHERE provider_user_id=$1 FOR SHARE",
        )
        .bind(charge.provider_user_id())
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?
        .ok_or(BoostyCreditError::UnlinkedPayer)?;

        let fingerprint = charge.fingerprint();
        let reference = charge.journal_reference();

        // Claim first: a concurrent retry for this external operation waits
        // on the UNIQUE index and cannot perform a second wallet movement.
        let claimed = sqlx::query_scalar::<_, String>(
            "INSERT INTO boosty_subscription_credits
              (provider_operation_id,provider_user_id,user_id,charge_kind,
               amount_minor,charge_fingerprint,wallet_reference)
             VALUES($1,$2,$3,$4,$5,$6,$7)
             ON CONFLICT (provider_operation_id) DO NOTHING
             RETURNING provider_operation_id",
        )
        .bind(charge.operation_id())
        .bind(charge.provider_user_id())
        .bind(user_id)
        .bind(charge.kind().as_str())
        .bind(charge.amount_minor())
        .bind(fingerprint.as_slice())
        .bind(&reference)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;

        if claimed.is_none() {
            let prior = sqlx::query(
                "SELECT provider_user_id,user_id,amount_minor,charge_kind,
                        charge_fingerprint,wallet_reference
                 FROM boosty_subscription_credits
                 WHERE provider_operation_id=$1 FOR UPDATE",
            )
            .bind(charge.operation_id())
            .fetch_one(&mut *tx)
            .await
            .map_err(unavailable)?;
            let prior_fingerprint: Vec<u8> =
                prior.try_get("charge_fingerprint").map_err(unavailable)?;
            let prior_ref: String = prior.try_get("wallet_reference").map_err(unavailable)?;
            if prior
                .try_get::<i64, _>("provider_user_id")
                .map_err(unavailable)?
                != charge.provider_user_id()
                || prior.try_get::<i64, _>("user_id").map_err(unavailable)? != user_id
                || prior
                    .try_get::<i64, _>("amount_minor")
                    .map_err(unavailable)?
                    != charge.amount_minor()
                || prior
                    .try_get::<String, _>("charge_kind")
                    .map_err(unavailable)?
                    != charge.kind().as_str()
                || prior_fingerprint.as_slice() != fingerprint
                || prior_ref != reference
            {
                return Err(BoostyCreditError::ConflictingEvidence);
            }
            let old_balance = sqlx::query_scalar::<_, i64>(
                "SELECT balance_after_minor
                 FROM reader_wallet_entries
                 WHERE user_id=$1 AND reference=$2
                   AND kind='verified_funding' AND delta_minor=$3",
            )
            .bind(user_id)
            .bind(&reference)
            .bind(charge.amount_minor())
            .fetch_optional(&mut *tx)
            .await
            .map_err(unavailable)?
            .ok_or(BoostyCreditError::Unavailable)?;
            tx.commit().await.map_err(unavailable)?;
            return Ok(BoostyCreditReceipt {
                user_id,
                amount_minor: charge.amount_minor(),
                balance_after_minor: old_balance,
                replayed: true,
            });
        }

        // The wallet may not exist for a new reader; it is created with zero.
        // All writes (claim, wallet and journal) commit or roll back together.
        sqlx::query(
            "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
             VALUES($1,0,'RUB') ON CONFLICT(user_id) DO NOTHING",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        let balance = sqlx::query_scalar::<_, i64>(
            "SELECT balance_minor FROM reader_wallet_accounts
             WHERE user_id=$1 AND currency='RUB' FOR UPDATE",
        )
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        let after = balance
            .checked_add(charge.amount_minor())
            .ok_or(BoostyCreditError::Overflow)?;
        sqlx::query(
            "UPDATE reader_wallet_accounts SET balance_minor=$2
             WHERE user_id=$1 AND currency='RUB'",
        )
        .bind(user_id)
        .bind(after)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        sqlx::query(
            "INSERT INTO reader_wallet_entries
              (user_id,kind,delta_minor,balance_after_minor,reference,purchase_id)
             VALUES($1,'verified_funding',$2,$3,$4,NULL)",
        )
        .bind(user_id)
        .bind(charge.amount_minor())
        .bind(after)
        .bind(&reference)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        // A deferred database trigger independently proves the journal matches
        // the claimed source. Crash/unknown commit outcome is resolved by replay.
        tx.commit().await.map_err(unavailable)?;
        Ok(BoostyCreditReceipt {
            user_id,
            amount_minor: charge.amount_minor(),
            balance_after_minor: after,
            replayed: false,
        })
    }
}
