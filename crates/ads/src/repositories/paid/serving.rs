use super::*;
use sha2::{Digest, Sha256};
use sqlx::Row;
use tor_contracts::exposure::PromotionSlot;

impl PgPaidAds {
    pub async fn cards(
        &self,
        a: Option<&AdActor>,
        slot: PromotionSlot,
        host: Option<i64>,
        flags: AdFlags,
        has_editorial: bool,
    ) -> Result<(bool, Vec<PaidCard>), AdError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        if let Some(a) = a {
            actor(&mut tx, a, false).await?;
        }
        let Some(policy) = sqlx::query("SELECT * FROM ad_slot_policies WHERE slot=$1")
            .bind(slot.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?
        else {
            return Ok((false, vec![]));
        };
        let first: bool = policy.try_get("paid_first").map_err(db)?;
        let max: i32 = policy.try_get("max_cards").map_err(db)?;
        if has_editorial && !first {
            return Ok((first, vec![]));
        }
        let mut cards = Vec::new();
        if flags.daily_serving
            && a.is_some()
            && policy.try_get::<bool, _>("daily_enabled").map_err(db)?
        {
            let rows=sqlx::query("SELECT c.id,c.book_id,b.title,r.daily_cap,r.hourly_cap,r.session_cap,r.min_interval_seconds FROM ad_legacy_campaigns c
    JOIN ad_daily_offer_revisions r ON r.offer_id=c.offer_id AND r.revision=c.offer_revision
    JOIN books b ON b.id=c.book_id AND b.owner_user_id=c.advertiser_id
    JOIN finance_ad_funding f ON f.intent_id=c.funding_intent_id AND f.status='funded'
    JOIN users advertiser ON advertiser.id=c.advertiser_id AND advertiser.account_state='active'
    WHERE c.status='funded' AND c.slot=$1 AND c.starts_at<=clock_timestamp() AND c.ends_at>clock_timestamp()
      AND ad_book_discoverable($2,c.book_id) AND ($3::BIGINT IS NULL OR
        (c.book_id<>$3 AND ad_book_discoverable($2,$3)))
    ORDER BY md5(c.id::TEXT||$2::TEXT||date_trunc('hour',now())::TEXT),c.id LIMIT $4")
    .bind(slot.as_str()).bind(a.map(|a|a.user_id)).bind(host).bind(16_i64)
    .fetch_all(&mut *tx).await.map_err(db)?;
            for row in rows {
                if cards.len() >= usize::try_from(max).map_err(|_| AdError::Invalid)? {
                    break;
                }
                let viewer = a.ok_or(AdError::Denied)?;
                if !daily_caps::allow(&mut tx, viewer, &row).await? {
                    continue;
                }
                cards.push(PaidCard {
                    product: "daily".into(),
                    campaign_id: row.try_get("id").map_err(db)?,
                    book_id: row.try_get("book_id").map_err(db)?,
                    title: row.try_get("title").map_err(db)?,
                    generation_id: None,
                    assignment_id: None,
                    opportunity_token: None,
                });
            }
        }
        if cards.is_empty()
            && flags.chapter_serving
            && policy.try_get::<bool, _>("chapter_enabled").map_err(db)?
        {
            if let Some(a) = a {
                let rows=sqlx::query("SELECT a.id,a.generation_id,a.campaign_id,a.promoted_book_id,b.title FROM ad_live_assignments a
      JOIN books b ON b.id=a.promoted_book_id JOIN finance_ad_funding f ON f.intent_id=a.funding_intent_id
      WHERE a.host_book_id=$1 AND a.publisher_id<>$2 AND a.advertiser_id<>$2
        AND f.status='funded' AND f.reserved_units<f.target_units
        AND ad_book_discoverable($2,a.host_book_id) AND ad_book_discoverable($2,a.promoted_book_id)
      ORDER BY a.rank LIMIT $3")
      .bind(host).bind(a.user_id).bind(i64::from(max)).fetch_all(&mut *tx).await.map_err(db)?;
                for row in rows {
                    let promoted: i64 = row.try_get("promoted_book_id").map_err(db)?;
                    if cards.iter().any(|card| card.book_id == promoted) {
                        continue;
                    }
                    cards.push(PaidCard {
                        product: "chapter".into(),
                        campaign_id: row.try_get("campaign_id").map_err(db)?,
                        book_id: row.try_get("promoted_book_id").map_err(db)?,
                        title: row.try_get("title").map_err(db)?,
                        generation_id: Some(row.try_get("generation_id").map_err(db)?),
                        assignment_id: Some(row.try_get("id").map_err(db)?),
                        opportunity_token: None,
                    });
                }
            }
        }
        tx.commit().await.map_err(db)?;
        Ok((first, cards))
    }
    pub async fn issue_opportunity(
        &self,
        a: &AdActor,
        assignment: i64,
        chapter: i64,
        token: &str,
    ) -> Result<(), AdError> {
        if token.len() != 43 || assignment <= 0 || chapter <= 0 {
            return Err(AdError::Invalid);
        }
        let hash = Sha256::digest(token.as_bytes());
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        actor(&mut tx, a, false).await?;
        // Bounded issuance budget, durable across replicas. This creates no billable
        // effect; the D reservation independently rechecks every live condition.
        let budget:bool=sqlx::query_scalar("SELECT count(*)<120 FROM (SELECT id FROM ad_viewability_opportunities
    WHERE viewer_id=$1 AND issued_at>clock_timestamp()-interval '1 minute' ORDER BY issued_at DESC LIMIT 120) q")
    .bind(a.user_id).fetch_one(&mut *tx).await.map_err(db)?;
        if !budget {
            return Err(AdError::CapReached);
        }
        let result=sqlx::query("INSERT INTO ad_viewability_opportunities(token_hash,assignment_id,viewer_id,session_hash,host_book_id,host_chapter_id)
    SELECT $1,a.id,$2,$3,a.host_book_id,c.id FROM ad_live_assignments a
    JOIN chapters c ON c.id=$4 AND c.book_id=a.host_book_id JOIN chapter_publications p ON p.chapter_id=c.id
    WHERE a.id=$5 AND a.publisher_id<>$2 AND a.advertiser_id<>$2
      AND c.visibility='public' AND c.access_level='free' AND c.price_amount=0 AND NOT c.admin_hold AND NOT c.is_deleted
      AND (c.reader_released_at IS NULL OR c.reader_released_at<=clock_timestamp())
      AND ad_book_discoverable($2,a.host_book_id) AND ad_book_discoverable($2,a.promoted_book_id)")
    .bind(hash.as_slice()).bind(a.user_id).bind(a.session_digest.as_slice()).bind(chapter).bind(assignment)
    .execute(&mut *tx).await.map_err(db)?;
        if result.rows_affected() != 1 {
            return Err(AdError::Denied);
        }
        tx.commit().await.map_err(db)
    }
}
