//! D-owned advertising authority. F campaign writes are the documented
//! ADS-03/06/09/11/12 atomicity exception; no other F state is mutated here.
mod cancel;
mod maintenance;
mod prepare;
mod publisher;
mod purchase;
mod reserve;
mod settlement;

use async_trait::async_trait;
use sqlx::{PgPool, Postgres, Transaction};
use tor_contracts::{ads::*, money::MoneyMinor};

#[derive(Clone)]
pub struct PgAdPosting {
    pub(super) pool: PgPool,
}
impl PgAdPosting {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
pub(super) fn db(error: sqlx::Error) -> AdError {
    match error.as_database_error().and_then(|v| v.code()).as_deref() {
        Some("P0R38") => AdError::Insufficient,
        Some("23505") => AdError::Conflict,
        Some("23503") => AdError::NotFound,
        Some("23514" | "22003" | "22007" | "22008" | "22P02") => AdError::Invalid,
        _ => AdError::Unavailable,
    }
}
pub(super) async fn limits(tx: &mut Transaction<'_, Postgres>) -> Result<(), AdError> {
    sqlx::query("SET LOCAL statement_timeout='5s'")
        .execute(&mut **tx)
        .await
        .map_err(db)?;
    sqlx::query("SET LOCAL lock_timeout='3s'")
        .execute(&mut **tx)
        .await
        .map_err(db)?;
    Ok(())
}
pub(super) async fn authorize(
    tx: &mut Transaction<'_, Postgres>,
    actor: &AdActor,
) -> Result<(), AdError> {
    let allowed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users u JOIN sessions s ON s.user_id=u.id
        WHERE u.id=$1 AND u.account_state='active' AND s.token_hash=$2
          AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp())",
    )
    .bind(actor.user_id)
    .bind(actor.session_digest.as_slice())
    .fetch_one(&mut **tx)
    .await
    .map_err(db)?;
    if !allowed {
        return Err(AdError::Denied);
    }
    Ok(())
}
pub(super) async fn lock_wallet(
    tx: &mut Transaction<'_, Postgres>,
    actor: &AdActor,
) -> Result<i64, AdError> {
    // Match checkout/payout: wallet -> user NO KEY UPDATE. Never retain an
    // actor-user SHARE lock while waiting for its wallet reservation guard.
    let balance: Option<i64> = sqlx::query_scalar(
        "SELECT balance_minor FROM reader_wallet_accounts
        WHERE user_id=$1 AND currency='RUB' FOR UPDATE",
    )
    .bind(actor.user_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(db)?;
    let balance = balance.ok_or(AdError::Insufficient)?;
    let user: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM users WHERE id=$1
        AND account_state='active' FOR NO KEY UPDATE",
    )
    .bind(actor.user_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(db)?;
    if user.is_none() {
        return Err(AdError::Denied);
    }
    let session: Option<i64> = sqlx::query_scalar(
        "SELECT user_id FROM sessions WHERE user_id=$1
        AND token_hash=$2 AND revoked_at IS NULL AND expires_at>clock_timestamp() FOR SHARE",
    )
    .bind(actor.user_id)
    .bind(actor.session_digest.as_slice())
    .fetch_optional(&mut **tx)
    .await
    .map_err(db)?;
    if session.is_none() {
        return Err(AdError::Denied);
    }
    Ok(balance)
}
pub(super) async fn entry(
    tx: &mut Transaction<'_, Postgres>,
    transaction: i64,
    account: &str,
    owner: Option<i64>,
    amount: i64,
) -> Result<(), AdError> {
    if amount != 0 {
        sqlx::query(
            "INSERT INTO finance_ad_entries(transaction_id,account,owner_user_id,delta_minor)
            VALUES($1,$2,$3,$4)",
        )
        .bind(transaction)
        .bind(account)
        .bind(owner)
        .bind(amount)
        .execute(&mut **tx)
        .await
        .map_err(db)?;
    }
    Ok(())
}
pub(super) async fn transaction(
    tx: &mut Transaction<'_, Postgres>,
    intent: i64,
    identity: &str,
    operation: &str,
    amount: i64,
    wallet_entry: Option<i64>,
) -> Result<i64, AdError> {
    sqlx::query_scalar("INSERT INTO finance_ad_transactions(identity,intent_id,operation,amount_minor,wallet_entry_id)
        VALUES($1,$2,$3,$4,$5) RETURNING id").bind(identity).bind(intent).bind(operation)
        .bind(amount).bind(wallet_entry).fetch_one(&mut **tx).await.map_err(db)
}

#[async_trait]
impl AdFundingPort for PgAdPosting {
    async fn purchase(&self, command: &AdPurchaseCommand) -> Result<AdPurchaseReceipt, AdError> {
        let intent = self.prepare(command).await?;
        let result = self.fund_once(command, intent).await;
        if result.is_err() {
            // Non-financial intent survives a failed money transaction. A
            // changed payload cannot reuse a failed/pending key on any replica.
            let _ = sqlx::query(
                "UPDATE finance_ad_intents SET status='failed'
                WHERE id=$1 AND status IN('pending','failed')",
            )
            .bind(intent)
            .execute(&self.pool)
            .await;
        }
        result
    }
    async fn reserve(&self, claim: &AdImpressionClaim) -> Result<AdReservation, AdError> {
        self.reserve_once(claim).await
    }
    async fn cancel(&self, command: &AdCancelCommand) -> Result<AdCancellation, AdError> {
        self.cancel_once(command).await
    }
    async fn settle(&self, campaign: AdCampaignId, limit: u32) -> Result<u32, AdError> {
        if limit == 0 || limit > 100 {
            return Err(AdError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        let result = settlement::chapter(&mut tx, campaign.get(), limit).await;
        match result {
            Ok(v) => {
                tx.commit().await.map_err(db)?;
                Ok(v)
            }
            Err(e) => {
                tx.rollback().await.map_err(db)?;
                Err(e)
            }
        }
    }
    async fn mature_publisher(&self, actor: &AdActor, limit: u32) -> Result<MoneyMinor, AdError> {
        self.mature_actor(actor, limit).await
    }
    async fn release_publisher(
        &self,
        actor: &AdActor,
        key: &str,
        limit: u32,
    ) -> Result<MoneyMinor, AdError> {
        self.release_once(actor, key, limit).await
    }
}
