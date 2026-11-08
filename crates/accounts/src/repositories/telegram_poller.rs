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
}
