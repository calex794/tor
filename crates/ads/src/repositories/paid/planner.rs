use super::*;
use sqlx::Row;

impl PgPaidAds {
    /// A transaction lock is the fence; the durable keyset cursor survives a
    /// process death. Incomplete generations never replace the active pointer.
    pub async fn plan_batch(&self, limit: u32) -> Result<(i64, bool), AdError> {
        if limit == 0 || limit > 100 {
            return Err(AdError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(48261420261116)")
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?;
        if !locked {
            return Err(AdError::Conflict);
        }
        let mut generation=sqlx::query("SELECT * FROM ad_delivery_generations WHERE status='building' ORDER BY id LIMIT 1 FOR UPDATE")
   .fetch_optional(&mut *tx).await.map_err(db)?;
        if generation.is_none() {
            let current=sqlx::query("SELECT g.id FROM ad_delivery_active a JOIN ad_delivery_generations g ON g.id=a.generation_id
     WHERE g.created_at>clock_timestamp()-interval '5 minutes' AND NOT EXISTS(SELECT 1 FROM ad_planner_control WHERE dirty)")
     .fetch_optional(&mut *tx).await.map_err(db)?;
            if let Some(current) = current {
                let id = current.try_get("id").map_err(db)?;
                tx.commit().await.map_err(db)?;
                return Ok((id, true));
            }
            sqlx::query("UPDATE ad_planner_control SET dirty=false")
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            generation=Some(sqlx::query("INSERT INTO ad_delivery_generations(host_ceiling,campaign_ceiling)
    VALUES(COALESCE((SELECT max(id) FROM books),0),COALESCE((SELECT max(id) FROM ad_chapter_campaigns),0)) RETURNING *")
    .fetch_one(&mut *tx).await.map_err(db)?);
        }
        let generation = generation.ok_or(AdError::Unavailable)?;
        let id: i64 = generation.try_get("id").map_err(db)?;
        let ceiling: i64 = generation.try_get("host_ceiling").map_err(db)?;
        let cursor: i64 = generation.try_get("host_cursor").map_err(db)?;
        let campaigns: i64 = generation.try_get("campaign_ceiling").map_err(db)?;
        let hosts: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM books WHERE id>$1 AND id<=$2 ORDER BY id LIMIT $3")
                .bind(cursor)
                .bind(ceiling)
                .bind(i64::from(limit))
                .fetch_all(&mut *tx)
                .await
                .map_err(db)?;
        for host in &hosts {
            // At most 200 candidate IDs per host, selected with indexed keysets. ACL
            // filtering cannot turn the request into a whole-campaign-table scan.
            let pivot = if campaigns > 0 {
                1 + host % campaigns
            } else {
                1
            };
            sqlx::query("WITH candidates AS MATERIALIZED (
      (SELECT id FROM ad_chapter_campaigns WHERE status='funded' AND id>=$2 AND id<=$3 ORDER BY id LIMIT 100)
      UNION ALL (SELECT id FROM ad_chapter_campaigns WHERE status='funded' AND id<$2 ORDER BY id LIMIT 100)
    ), choices AS (
      SELECT c.id,host.owner_user_id AS publisher_id,row_number() OVER(ORDER BY (c.id<$2),c.id) AS rank
      FROM candidates candidate JOIN ad_chapter_campaigns c ON c.id=candidate.id
      JOIN finance_ad_funding f ON f.intent_id=c.funding_intent_id AND f.status='funded' AND f.reserved_units<f.target_units
      JOIN books host ON host.id=$1 JOIN rec_materializable_books vh ON vh.id=host.id
      JOIN ad_publisher_settings publisher ON publisher.user_id=host.owner_user_id AND publisher.enabled
      JOIN books promoted ON promoted.id=c.promoted_book_id AND promoted.owner_user_id=c.advertiser_id
      JOIN rec_materializable_books vp ON vp.id=promoted.id
      JOIN users advertiser ON advertiser.id=c.advertiser_id AND advertiser.account_state='active'
      JOIN users publisher_account ON publisher_account.id=host.owner_user_id AND publisher_account.account_state='active'
      WHERE host.id<>promoted.id AND host.owner_user_id<>c.advertiser_id
    ) INSERT INTO ad_delivery_assignments(generation_id,host_book_id,publisher_id,campaign_id,rank)
      SELECT $4,$1,publisher_id,id,rank::INTEGER FROM choices WHERE rank<=4
      ON CONFLICT(generation_id,host_book_id,campaign_id) DO NOTHING")
    .bind(host).bind(pivot).bind(campaigns).bind(id).execute(&mut *tx).await.map_err(db)?;
        }
        let cursor = hosts.last().copied().unwrap_or(ceiling);
        let complete = hosts.len() < usize::try_from(limit).map_err(|_| AdError::Invalid)?
            || cursor >= ceiling;
        sqlx::query("UPDATE ad_delivery_generations SET host_cursor=$2,status=CASE WHEN $3 THEN 'ready' ELSE 'building' END WHERE id=$1")
    .bind(id).bind(cursor).bind(complete).execute(&mut *tx).await.map_err(db)?;
        if complete {
            sqlx::query("UPDATE ad_delivery_generations SET status='retired' WHERE id=(SELECT generation_id FROM ad_delivery_active)")
    .execute(&mut *tx).await.map_err(db)?;
            sqlx::query(
                "UPDATE ad_delivery_generations SET status='active' WHERE id=$1 AND status='ready'",
            )
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
            sqlx::query(
                "INSERT INTO ad_delivery_active(singleton,generation_id) VALUES(true,$1)
    ON CONFLICT(singleton) DO UPDATE SET generation_id=$1",
            )
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        }
        tx.commit().await.map_err(db)?;
        Ok((id, complete))
    }
}
