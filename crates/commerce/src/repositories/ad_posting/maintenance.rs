//! Trusted, bounded recovery of already posted obligations. No fresh debit.
use super::*;
use sqlx::Row;

impl PgAdPosting {
    pub async fn resume_cancellations(&self, limit: u32) -> Result<u32, AdError> {
        if limit == 0 || limit > 100 {
            return Err(AdError::Invalid);
        }
        let rows=sqlx::query("SELECT f.intent_id,f.product,f.campaign_id,c.actor_id,c.command_key,c.expected_revision,c.reason
    FROM finance_ad_cancel_intents c JOIN finance_ad_funding f ON f.intent_id=c.intent_id
    WHERE f.status='funded' ORDER BY c.intent_id LIMIT $1")
    .bind(i64::from(limit)).fetch_all(&self.pool).await.map_err(db)?;
        let mut complete = 0;
        for row in rows {
            let command = AdCancelCommand {
                actor: AdActor {
                    user_id: row.try_get("actor_id").map_err(db)?,
                    session_digest: [0; 32],
                },
                command_key: row.try_get("command_key").map_err(db)?,
                expected_revision: row.try_get("expected_revision").map_err(db)?,
                reason: row.try_get("reason").map_err(db)?,
                campaign_id: AdCampaignId::new(row.try_get("campaign_id").map_err(db)?)?,
                product: match row.try_get::<String, _>("product").map_err(db)?.as_str() {
                    "daily" => AdProduct::Daily,
                    "chapter" => AdProduct::Chapter,
                    _ => return Err(AdError::Invalid),
                },
            };
            if !self
                .finish_cancel(&command, row.try_get("intent_id").map_err(db)?)
                .await?
                .pending
            {
                complete += 1;
            }
        }
        Ok(complete)
    }
    pub async fn settle_due(&self, limit: u32) -> Result<u32, AdError> {
        if limit == 0 || limit > 100 {
            return Err(AdError::Invalid);
        }
        let campaigns: Vec<i64> = sqlx::query_scalar(
            "SELECT campaign_id FROM ad_impression_reservations
    WHERE settled_at IS NULL ORDER BY campaign_id,ordinal LIMIT $1",
        )
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        let mut previous = None;
        let mut count = 0;
        for campaign in campaigns {
            if previous == Some(campaign) {
                continue;
            }
            previous = Some(campaign);
            count += self.settle(AdCampaignId::new(campaign)?, limit).await?;
        }
        let daily:Vec<i64>=sqlx::query_scalar("SELECT f.intent_id FROM finance_ad_funding f JOIN finance_ad_intents i ON i.id=f.intent_id
    JOIN ad_legacy_campaigns c ON c.funding_intent_id=f.intent_id AND c.status='funded'
    WHERE f.product='daily' AND f.status='funded' AND f.reserved_units<LEAST(i.total_days,
      GREATEST(0,floor(extract(epoch FROM clock_timestamp()-i.starts_at)/86400)))
    ORDER BY f.intent_id LIMIT $1")
    .bind(i64::from(limit)).fetch_all(&self.pool).await.map_err(db)?;
        for intent in daily {
            let mut tx = self.pool.begin().await.map_err(db)?;
            limits(&mut tx).await?;
            settlement::daily(&mut tx, intent, false).await?;
            sqlx::query("UPDATE ad_legacy_campaigns c SET status='completed',revision=revision+1 FROM finance_ad_funding f
     WHERE f.intent_id=c.funding_intent_id AND f.intent_id=$1 AND c.status='funded' AND f.reserved_units=f.target_units")
     .bind(intent).execute(&mut *tx).await.map_err(db)?;
            tx.commit().await.map_err(db)?;
            count += 1;
        }
        Ok(count)
    }
}
