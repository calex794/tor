use super::*;
use crate::domain::ad_money::AdPricing;
use sha2::{Digest, Sha256};
use sqlx::Row;

impl PgAdPosting {
    pub(super) async fn reserve_once(
        &self,
        claim: &AdImpressionClaim,
    ) -> Result<AdReservation, AdError> {
        if !(60..=100).contains(&claim.visible_percent)
            || claim.duration_ms < 1000
            || claim.duration_ms > 300_000
            || claim.viewability_token.len() != 43
        {
            return Err(AdError::Invalid);
        }
        crate::domain::money::IdempotencyKey::parse(&claim.viewability_token)
            .map_err(|_| AdError::Invalid)?;
        let hash = Sha256::digest(claim.viewability_token.as_bytes());
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        let result=async {
            authorize(&mut tx,&claim.actor).await?;
            let opportunity=sqlx::query("SELECT o.id,o.assignment_id,assignment.campaign_id,c.funding_intent_id,
                  assignment.publisher_id,c.promoted_book_id,o.host_book_id,o.host_chapter_id,
                  o.expires_at>clock_timestamp() AS unexpired,
                  o.issued_at+interval '1 second'<=clock_timestamp() AS elapsed
                FROM ad_viewability_opportunities o JOIN ad_delivery_assignments assignment ON assignment.id=o.assignment_id
                JOIN ad_chapter_campaigns c ON c.id=assignment.campaign_id
                WHERE o.token_hash=$1 AND o.viewer_id=$2 AND o.session_hash=$3 AND o.host_book_id=assignment.host_book_id
                  AND assignment.campaign_id=$4 AND assignment.generation_id=$5 AND assignment.id=$6")
                .bind(hash.as_slice()).bind(claim.actor.user_id).bind(claim.actor.session_digest.as_slice())
                .bind(claim.context.campaign_id.get()).bind(claim.context.generation_id.get()).bind(claim.context.assignment_id.get())
                .fetch_optional(&mut *tx).await.map_err(db)?.ok_or(AdError::Denied)?;
            let intent:i64=opportunity.try_get("funding_intent_id").map_err(db)?;
            // One durable lock serializes dedup, caps, hard target and money.
            let funding=sqlx::query("SELECT f.*,i.daily_cap,i.hourly_cap,i.session_cap FROM finance_ad_funding f
                JOIN finance_ad_intents i ON i.id=f.intent_id WHERE f.intent_id=$1 FOR NO KEY UPDATE OF f")
                .bind(intent).fetch_one(&mut *tx).await.map_err(db)?;
            let opportunity_id:i64=opportunity.try_get("id").map_err(db)?;
            if let Some(saved)=sqlx::query("SELECT id,campaign_id,ordinal,gross_minor FROM ad_impression_reservations
                WHERE opportunity_id=$1").bind(opportunity_id).fetch_optional(&mut *tx).await.map_err(db)? {
                return reservation(&saved,true);
            }
            if !opportunity.try_get::<bool,_>("unexpired").map_err(db)? {return Err(AdError::Expired);}
            if !opportunity.try_get::<bool,_>("elapsed").map_err(db)? {return Err(AdError::Invalid);}
            // Lock mutable authority in this money transaction, then re-read
            // the canonical gate. No process-local cache can authorize billing.
            sqlx::query("SELECT id FROM books WHERE id=ANY($1::BIGINT[]) ORDER BY id FOR SHARE")
                .bind(vec![opportunity.try_get::<i64,_>("host_book_id").map_err(db)?,opportunity.try_get::<i64,_>("promoted_book_id").map_err(db)?])
                .fetch_all(&mut *tx).await.map_err(db)?;
            sqlx::query("SELECT id FROM chapters WHERE id=$1 FOR SHARE")
                .bind(opportunity.try_get::<i64,_>("host_chapter_id").map_err(db)?).fetch_one(&mut *tx).await.map_err(db)?;
            sqlx::query("SELECT id FROM users WHERE id=ANY($1::BIGINT[]) ORDER BY id FOR SHARE")
                .bind(vec![claim.actor.user_id,opportunity.try_get::<i64,_>("publisher_id").map_err(db)?,funding.try_get::<i64,_>("advertiser_id").map_err(db)?])
                .fetch_all(&mut *tx).await.map_err(db)?;
            sqlx::query("SELECT user_id FROM sessions WHERE token_hash=$1 FOR SHARE").bind(claim.actor.session_digest.as_slice())
                .fetch_one(&mut *tx).await.map_err(db)?;
            sqlx::query("SELECT user_id FROM user_level_grants WHERE user_id=$1 AND is_active FOR SHARE")
                .bind(claim.actor.user_id).fetch_all(&mut *tx).await.map_err(db)?;
            sqlx::query("SELECT generation_id FROM ad_delivery_active FOR SHARE").fetch_all(&mut *tx).await.map_err(db)?;
            sqlx::query("SELECT user_id FROM ad_publisher_settings WHERE user_id=$1 FOR SHARE")
                .bind(opportunity.try_get::<i64,_>("publisher_id").map_err(db)?).fetch_one(&mut *tx).await.map_err(db)?;
            let live:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ad_live_assignments a
                JOIN chapters c ON c.book_id=a.host_book_id AND c.id=$4
                JOIN chapter_publications published ON published.chapter_id=c.id
                WHERE a.id=$1 AND a.publisher_id<>$2 AND a.advertiser_id<>$2
                  AND c.visibility='public' AND c.access_level='free' AND c.price_amount=0
                  AND NOT c.admin_hold AND NOT c.is_deleted
                  AND (c.reader_released_at IS NULL OR c.reader_released_at<=clock_timestamp())
                  AND ad_book_discoverable($2,a.host_book_id) AND ad_book_discoverable($2,a.promoted_book_id)
                  AND EXISTS(SELECT 1 FROM sessions s WHERE s.user_id=$2 AND s.token_hash=$3
                    AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp()))")
                .bind(opportunity.try_get::<i64,_>("assignment_id").map_err(db)?).bind(claim.actor.user_id)
                .bind(claim.actor.session_digest.as_slice()).bind(opportunity.try_get::<i64,_>("host_chapter_id").map_err(db)?)
                .fetch_one(&mut *tx).await.map_err(db)?;
            if !live {return Err(AdError::Denied);}
            let reserved:i64=funding.try_get("reserved_units").map_err(db)?;
            let target:i64=funding.try_get("target_units").map_err(db)?;
            if funding.try_get::<String,_>("status").map_err(db)?!="funded" || reserved>=target {return Err(AdError::CapReached);}
            let campaign:i64=funding.try_get("campaign_id").map_err(db)?;
            let mut subject=Sha256::new(); subject.update(b"tor.ad.frequency.user.v1");subject.update(campaign.to_be_bytes());
            subject.update(claim.actor.user_id.to_be_bytes());
            let user_hash:[u8;32]=subject.finalize().into();
            for (kind,subject,limit) in [
                ("day",user_hash.as_slice(),funding.try_get::<i32,_>("daily_cap").map_err(db)?),
                ("hour",user_hash.as_slice(),funding.try_get::<i32,_>("hourly_cap").map_err(db)?),
                ("session",claim.actor.session_digest.as_slice(),funding.try_get::<i32,_>("session_cap").map_err(db)?),
            ] {
                let accepted:Option<i32>=sqlx::query_scalar("INSERT INTO ad_frequency_buckets
                    (campaign_id,subject_hash,bucket,window_start,count,expires_at)
                    VALUES($1,$2,$3,CASE $3 WHEN 'day' THEN date_trunc('day',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                      WHEN 'hour' THEN date_trunc('hour',now()) ELSE 'epoch'::timestamptz END,1,
                      CASE $3 WHEN 'session' THEN (SELECT expires_at FROM sessions WHERE token_hash=$2)
                        ELSE now()+interval '2 days' END)
                    ON CONFLICT(campaign_id,subject_hash,bucket,window_start)
                    DO UPDATE SET count=ad_frequency_buckets.count+1 WHERE ad_frequency_buckets.count<$4 RETURNING count")
                    .bind(campaign).bind(subject).bind(kind).bind(limit).fetch_optional(&mut *tx).await.map_err(db)?;
                if accepted.is_none() {return Err(AdError::CapReached);}
            }
            let pricing=AdPricing::new(MoneyMinor::new(funding.try_get("budget_minor").map_err(db)?).map_err(|_|AdError::Invalid)?,
                target,i64::from(funding.try_get::<i32,_>("publisher_share_bps").map_err(db)?))?;
            let delta=pricing.next(reserved+1)?;
            let total=pricing.cumulative(reserved+1)?;
            let posting=transaction(&mut tx,intent,&format!("ads:reserve:{intent}:{}",reserved+1),"reserve",delta.gross.get(),None).await?;
            entry(&mut tx,posting,"escrow",None,-delta.gross.get()).await?;
            entry(&mut tx,posting,"reserved",None,delta.gross.get()).await?;
            sqlx::query("UPDATE finance_ad_funding SET reserved_units=$2,reserved_minor=$3 WHERE intent_id=$1")
                .bind(intent).bind(reserved+1).bind(total.gross.get()).execute(&mut *tx).await.map_err(db)?;
            let saved=sqlx::query("INSERT INTO ad_impression_reservations(opportunity_id,campaign_id,intent_id,
                ordinal,publisher_id,gross_minor,publisher_minor,platform_minor,reservation_transaction_id)
                VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING id,campaign_id,ordinal,gross_minor")
                .bind(opportunity_id).bind(campaign).bind(intent).bind(reserved+1)
                .bind(opportunity.try_get::<i64,_>("publisher_id").map_err(db)?).bind(delta.gross.get())
                .bind(delta.publisher.get()).bind(delta.platform.get()).bind(posting).fetch_one(&mut *tx).await.map_err(db)?;
            sqlx::query("INSERT INTO ad_campaign_stats(campaign_id,accepted) VALUES($1,1)
                ON CONFLICT(campaign_id) DO UPDATE SET accepted=ad_campaign_stats.accepted+1")
                .bind(campaign).execute(&mut *tx).await.map_err(db)?;
            sqlx::query("INSERT INTO ad_delivery_hourly_stats(campaign_id,hour_start,accepted)
                VALUES($1,date_trunc('hour',now()),1) ON CONFLICT(campaign_id,hour_start)
                DO UPDATE SET accepted=ad_delivery_hourly_stats.accepted+1")
                .bind(campaign).execute(&mut *tx).await.map_err(db)?;
            reservation(&saved,false)
        }.await;
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
}
fn reservation(row: &sqlx::postgres::PgRow, replayed: bool) -> Result<AdReservation, AdError> {
    Ok(AdReservation {
        impression_id: AdImpressionId::new(row.try_get("id").map_err(db)?)?,
        campaign_id: AdCampaignId::new(row.try_get("campaign_id").map_err(db)?)?,
        ordinal: row.try_get("ordinal").map_err(db)?,
        gross: MoneyMinor::new(row.try_get("gross_minor").map_err(db)?)
            .map_err(|_| AdError::Invalid)?,
        replayed,
    })
}
