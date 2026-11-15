use super::*;
use sha2::{Digest, Sha256};
use sqlx::Row;

impl PgPaidAds {
    pub async fn accept_purchase_event(&self, id: &str, epoch: i64) -> Result<(), AdError> {
        if id.len() != 36 || epoch <= 0 {
            return Err(AdError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        let row=sqlx::query("SELECT f.campaign_id,f.product FROM outbox_events e
    JOIN finance_ad_funding f ON e.dedupe_key='ads:fund:'||f.intent_id
    WHERE e.id=$1::UUID AND e.event_type='AdCampaignPurchased' AND e.schema_version=1
      AND e.lease_epoch=$2 AND e.lease_until>clock_timestamp() AND e.dead_at IS NULL AND e.delivered_at IS NULL
      AND e.payload=jsonb_build_object('product',f.product,'campaign_id',f.campaign_id,'intent_id',f.intent_id)
      AND e.required_consumers @> ARRAY['ads_aggregate']::TEXT[] FOR UPDATE OF e")
    .bind(id).bind(epoch).fetch_optional(&mut *tx).await.map_err(db)?.ok_or(AdError::Conflict)?;
        if row.try_get::<String, _>("product").map_err(db)? == "chapter" {
            sqlx::query("INSERT INTO ad_campaign_stats(campaign_id) VALUES($1) ON CONFLICT(campaign_id) DO NOTHING")
    .bind(row.try_get::<i64,_>("campaign_id").map_err(db)?).execute(&mut *tx).await.map_err(db)?;
        }
        sqlx::query("INSERT INTO ad_planner_control(singleton,dirty) VALUES(true,true) ON CONFLICT(singleton) DO UPDATE SET dirty=true")
    .execute(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)
    }
    pub async fn click(&self, a: &AdActor, token: &str) -> Result<i64, AdError> {
        self.record_navigation(a, token, None).await
    }
    pub async fn open(&self, a: &AdActor, token: &str, book: i64) -> Result<(), AdError> {
        self.record_navigation(a, token, Some(book))
            .await
            .map(|_| ())
    }
    async fn record_navigation(
        &self,
        a: &AdActor,
        token: &str,
        open: Option<i64>,
    ) -> Result<i64, AdError> {
        if token.len() != 43 {
            return Err(AdError::Invalid);
        }
        let digest = Sha256::digest(token.as_bytes());
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        actor(&mut tx, a, false).await?;
        let row=sqlx::query("SELECT o.id,c.id AS campaign,c.promoted_book_id FROM ad_viewability_opportunities o
    JOIN ad_delivery_assignments assignment ON assignment.id=o.assignment_id
    JOIN ad_chapter_campaigns c ON c.id=assignment.campaign_id
    WHERE o.token_hash=$1 AND o.viewer_id=$2 AND o.session_hash=$3 AND o.issued_at>clock_timestamp()-interval '30 minutes'
      AND ad_book_discoverable($2,c.promoted_book_id)
      AND ($4::BIGINT IS NULL OR (c.promoted_book_id=$4 AND EXISTS(SELECT 1 FROM ad_campaign_attribution event
        WHERE event.opportunity_id=o.id AND event.action='click')))")
    .bind(digest.as_slice()).bind(a.user_id).bind(a.session_digest.as_slice()).bind(open)
    .fetch_optional(&mut *tx).await.map_err(db)?.ok_or(AdError::Denied)?;
        let opportunity: i64 = row.try_get("id").map_err(db)?;
        let campaign: i64 = row.try_get("campaign").map_err(db)?;
        let action = if open.is_some() { "open" } else { "click" };
        let inserted = sqlx::query(
            "INSERT INTO ad_campaign_attribution(opportunity_id,action) VALUES($1,$2)
    ON CONFLICT(opportunity_id,action) DO NOTHING",
        )
        .bind(opportunity)
        .bind(action)
        .execute(&mut *tx)
        .await
        .map_err(db)?
        .rows_affected();
        if inserted == 1 {
            sqlx::query("INSERT INTO ad_campaign_stats(campaign_id) VALUES($1) ON CONFLICT(campaign_id) DO NOTHING")
                .bind(campaign).execute(&mut *tx).await.map_err(db)?;
            let update = if open.is_some() {
                "UPDATE ad_campaign_stats SET opens=opens+1 WHERE campaign_id=$1"
            } else {
                "UPDATE ad_campaign_stats SET clicks=clicks+1 WHERE campaign_id=$1"
            };
            sqlx::query(update)
                .bind(campaign)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
        }
        if open.is_none() {
            sqlx::query("INSERT INTO ad_attribution_pending(opportunity_id) VALUES($1) ON CONFLICT(opportunity_id) DO NOTHING")
    .bind(opportunity).execute(&mut *tx).await.map_err(db)?;
        }
        let book = row.try_get("promoted_book_id").map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(book)
    }
    /// Reconcile only short-lived ad contexts against committed C/D facts.
    /// There is no client endpoint accepting a claimed conversion.
    pub async fn reconcile_outcomes(&self, limit: u32) -> Result<u32, AdError> {
        if limit == 0 || limit > 100 {
            return Err(AdError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        let rows=sqlx::query("SELECT q.opportunity_id,o.viewer_id,c.id AS campaign,c.promoted_book_id,
    event.recorded_at AS clicked_at,q.expires_at<clock_timestamp() AS expired
    FROM ad_attribution_pending q JOIN ad_viewability_opportunities o ON o.id=q.opportunity_id
    JOIN ad_delivery_assignments assignment ON assignment.id=o.assignment_id
    JOIN ad_chapter_campaigns c ON c.id=assignment.campaign_id
    JOIN ad_campaign_attribution event ON event.opportunity_id=o.id AND event.action='click'
    WHERE q.next_check_at<=clock_timestamp() ORDER BY q.next_check_at,q.opportunity_id LIMIT $1 FOR UPDATE OF q SKIP LOCKED")
    .bind(i64::from(limit)).fetch_all(&mut *tx).await.map_err(db)?;
        for row in &rows {
            let opportunity: i64 = row.try_get("opportunity_id").map_err(db)?;
            let viewer: i64 = row.try_get("viewer_id").map_err(db)?;
            let book: i64 = row.try_get("promoted_book_id").map_err(db)?;
            for (action,proof,update) in [
    ("read","SELECT EXISTS(SELECT 1 FROM reader_chapter_completions WHERE user_id=$1 AND book_id=$2 AND completed_at>=$3::TEXT::TIMESTAMPTZ AND completed_at<=$3::TEXT::TIMESTAMPTZ+interval '30 minutes')",
      "UPDATE ad_campaign_stats SET reads=reads+1 WHERE campaign_id=$1"),
    ("library","SELECT EXISTS(SELECT 1 FROM reader_library_items WHERE user_id=$1 AND book_id=$2 AND created_at>=$3::TEXT::TIMESTAMPTZ AND created_at<=$3::TEXT::TIMESTAMPTZ+interval '30 minutes')",
      "UPDATE ad_campaign_stats SET library_adds=library_adds+1 WHERE campaign_id=$1"),
    ("purchase","SELECT EXISTS(SELECT 1 FROM chapter_purchase_facts p JOIN chapters ch ON ch.id=p.chapter_id WHERE p.user_id=$1 AND ch.book_id=$2 AND p.acquired_at>=$3::TEXT::TIMESTAMPTZ AND p.acquired_at<=$3::TEXT::TIMESTAMPTZ+interval '30 minutes' AND NOT EXISTS(SELECT 1 FROM chapter_purchase_revocations r WHERE r.purchase_id=p.id))",
      "UPDATE ad_campaign_stats SET purchases=purchases+1 WHERE campaign_id=$1"),
   ] {
    // Text is emitted by PostgreSQL, not supplied by a client timestamp.
    let clicked:String=sqlx::query_scalar("SELECT recorded_at::TEXT FROM ad_campaign_attribution WHERE opportunity_id=$1 AND action='click'")
      .bind(opportunity).fetch_one(&mut *tx).await.map_err(db)?;
    let exists:bool=sqlx::query_scalar(proof).bind(viewer).bind(book).bind(clicked).fetch_one(&mut *tx).await.map_err(db)?;
    if exists {
     let inserted=sqlx::query("INSERT INTO ad_campaign_attribution(opportunity_id,action) VALUES($1,$2)
       ON CONFLICT(opportunity_id,action) DO NOTHING").bind(opportunity).bind(action).execute(&mut *tx).await.map_err(db)?.rows_affected();
     if inserted==1 {sqlx::query(update).bind(row.try_get::<i64,_>("campaign").map_err(db)?).execute(&mut *tx).await.map_err(db)?;}
    }
   }
            if row.try_get::<bool, _>("expired").map_err(db)? {
                sqlx::query("DELETE FROM ad_attribution_pending WHERE opportunity_id=$1")
                    .bind(opportunity)
                    .execute(&mut *tx)
                    .await
                    .map_err(db)?;
            } else {
                sqlx::query("UPDATE ad_attribution_pending SET next_check_at=clock_timestamp()+interval '30 seconds' WHERE opportunity_id=$1")
      .bind(opportunity).execute(&mut *tx).await.map_err(db)?;
            }
        }
        tx.commit().await.map_err(db)?;
        u32::try_from(rows.len()).map_err(|_| AdError::Invalid)
    }
}
