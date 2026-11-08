//! A/S004: database-timed, fenced lease for Telegram long polling.
use super::PgAccounts;

impl PgAccounts {
    /// A conflict can only be won after the previous owner's lease expires.
    pub async fn claim_telegram_poller(
        &self,
        bot_id: i64,
        owner: &[u8; 32],
    ) -> Result<Option<i64>, sqlx::Error> {
        if bot_id <= 0 {
            return Ok(None);
        }
        sqlx::query_scalar(
            "INSERT INTO account_telegram_poller_leases
             (bot_id, owner_digest, expires_at)
             VALUES ($1, $2, clock_timestamp() + INTERVAL '90 seconds')
             ON CONFLICT (bot_id) DO UPDATE
             SET owner_digest = EXCLUDED.owner_digest,
                 lease_epoch = account_telegram_poller_leases.lease_epoch + 1,
                 expires_at = EXCLUDED.expires_at
             WHERE account_telegram_poller_leases.expires_at <= clock_timestamp()
             RETURNING lease_epoch",
        )
        .bind(bot_id)
        .bind(owner.as_slice())
        .fetch_optional(self.pool())
        .await
    }

    /// The owner must renew its exact generation before polling or handling
    /// messages. Suspended, expired or superseded owners cannot renew.
    pub async fn renew_telegram_poller(
        &self,
        bot_id: i64,
        owner: &[u8; 32],
        epoch: i64,
    ) -> Result<bool, sqlx::Error> {
        if bot_id <= 0 || epoch <= 0 {
            return Ok(false);
        }
        let renewed: Option<i64> = sqlx::query_scalar(
            "UPDATE account_telegram_poller_leases
             SET expires_at=clock_timestamp() + INTERVAL '90 seconds'
             WHERE bot_id=$1 AND owner_digest=$2 AND lease_epoch=$3
               AND expires_at>clock_timestamp()
             RETURNING lease_epoch",
        )
        .bind(bot_id)
        .bind(owner.as_slice())
        .bind(epoch)
        .fetch_optional(self.pool())
        .await?;
        Ok(renewed.is_some())
    }

    /// Expire rather than delete so the next owner gets a higher epoch.
    /// A stale owner's release never terminates a successor's lease.
    pub async fn release_telegram_poller(
        &self,
        bot_id: i64,
        owner: &[u8; 32],
        epoch: i64,
    ) -> Result<bool, sqlx::Error> {
        if bot_id <= 0 || epoch <= 0 {
            return Ok(false);
        }
        let changed: Option<i64> = sqlx::query_scalar(
            "UPDATE account_telegram_poller_leases
             SET expires_at=clock_timestamp()
             WHERE bot_id=$1 AND owner_digest=$2 AND lease_epoch=$3
               AND expires_at>clock_timestamp()
             RETURNING lease_epoch",
        )
        .bind(bot_id)
        .bind(owner.as_slice())
        .bind(epoch)
        .fetch_optional(self.pool())
        .await?;
        Ok(changed.is_some())
    }
    /// The receipt and the lease-generation check share a locked SQL statement.
    /// None means lost lease; Some(false) means an existing receipt.
    pub async fn acknowledge_telegram_update_fenced(
        &self,
        bot_id: i64,
        owner: &[u8; 32],
        epoch: i64,
        update_id: i64,
    ) -> Result<Option<bool>, sqlx::Error> {
        if bot_id <= 0 || epoch <= 0 || update_id < 0 {
            return Ok(None);
        }
        let (active, inserted): (bool, bool) = sqlx::query_as(
            "WITH active_lease AS MATERIALIZED (
               SELECT bot_id FROM account_telegram_poller_leases
               WHERE bot_id=$1 AND owner_digest=$2 AND lease_epoch=$3
                 AND expires_at>clock_timestamp()
               FOR SHARE
             ), receipt AS (
               INSERT INTO account_telegram_bot_updates(update_id)
               SELECT $4 FROM active_lease
               ON CONFLICT(update_id) DO NOTHING RETURNING update_id
             )
             SELECT EXISTS(SELECT 1 FROM active_lease),
                    EXISTS(SELECT 1 FROM receipt)",
        )
        .bind(bot_id)
        .bind(owner.as_slice())
        .bind(epoch)
        .bind(update_id)
        .fetch_one(self.pool())
        .await?;
        Ok(active.then_some(inserted))
    }
}
