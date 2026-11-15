use super::*;
impl PgPaidAds {
    pub async fn cleanup(&self) -> Result<(), AdError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        sqlx::query("WITH expired AS (SELECT campaign_id,subject_hash,bucket,window_start FROM ad_frequency_buckets
    WHERE expires_at<clock_timestamp() ORDER BY expires_at LIMIT 512 FOR UPDATE SKIP LOCKED)
    DELETE FROM ad_frequency_buckets b USING expired e WHERE (b.campaign_id,b.subject_hash,b.bucket,b.window_start)
      =(e.campaign_id,e.subject_hash,e.bucket,e.window_start)")
    .execute(&mut *tx).await.map_err(db)?;
        sqlx::query("WITH expired AS (SELECT id FROM ad_viewability_opportunities WHERE expires_at<clock_timestamp()-interval '1 day'
    AND viewer_id IS NOT NULL ORDER BY expires_at,id LIMIT 64 FOR UPDATE SKIP LOCKED)
    UPDATE ad_viewability_opportunities o SET viewer_id=NULL,session_hash=NULL FROM expired e WHERE o.id=e.id")
    .execute(&mut *tx).await.map_err(db)?;
        sqlx::query("WITH expired AS (SELECT o.id FROM ad_viewability_opportunities o WHERE o.expires_at<clock_timestamp()-interval '1 day'
    AND NOT EXISTS(SELECT 1 FROM ad_impression_reservations r WHERE r.opportunity_id=o.id)
    AND NOT EXISTS(SELECT 1 FROM ad_campaign_attribution a WHERE a.opportunity_id=o.id)
    AND NOT EXISTS(SELECT 1 FROM ad_attribution_pending a WHERE a.opportunity_id=o.id)
    ORDER BY o.expires_at,o.id LIMIT 64 FOR UPDATE SKIP LOCKED)
    DELETE FROM ad_viewability_opportunities o USING expired e WHERE o.id=e.id")
    .execute(&mut *tx).await.map_err(db)?;
        sqlx::query("WITH expired AS (SELECT campaign_id,subject_hash,bucket,window_start FROM ad_daily_frequency_buckets
          WHERE expires_at<clock_timestamp() ORDER BY expires_at LIMIT 512 FOR UPDATE SKIP LOCKED)
          DELETE FROM ad_daily_frequency_buckets b USING expired e WHERE (b.campaign_id,b.subject_hash,b.bucket,b.window_start)
            =(e.campaign_id,e.subject_hash,e.bucket,e.window_start)").execute(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)
    }
}
