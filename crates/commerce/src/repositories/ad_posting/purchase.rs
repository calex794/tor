use super::*;
use sqlx::Row;

impl PgAdPosting {
    pub(super) async fn fund_once(
        &self,
        command: &AdPurchaseCommand,
        intent: i64,
    ) -> Result<AdPurchaseReceipt, AdError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        let result=async {
            let row=sqlx::query("SELECT * FROM finance_ad_intents WHERE id=$1 AND advertiser_id=$2 FOR UPDATE")
                .bind(intent).bind(command.actor.user_id).fetch_optional(&mut *tx).await.map_err(db)?.ok_or(AdError::Denied)?;
            authorize(&mut tx,&command.actor).await?;
            if row.try_get::<String,_>("status").map_err(db)?=="posted" {
                let saved=sqlx::query("SELECT f.campaign_id,f.budget_minor,e.balance_after_minor
                    FROM finance_ad_funding f JOIN reader_wallet_entries e ON e.id=f.wallet_entry_id
                    WHERE f.intent_id=$1 AND f.advertiser_id=$2 AND e.user_id=$2
                      AND e.kind='ad_funding_debit' AND e.delta_minor=-f.budget_minor
                      AND e.reference='ads:fund:'||f.intent_id")
                    .bind(intent).bind(command.actor.user_id).fetch_one(&mut *tx).await.map_err(db)?;
                return Ok(AdPurchaseReceipt {campaign_id:AdCampaignId::new(saved.try_get("campaign_id").map_err(db)?)?,
                    product:command.purchase.product(),charged:MoneyMinor::new(saved.try_get("budget_minor").map_err(db)?).map_err(|_|AdError::Invalid)?,
                    balance_after:MoneyMinor::new(saved.try_get("balance_after_minor").map_err(db)?).map_err(|_|AdError::Invalid)?,replayed:true});
            }
            let price:i64=row.try_get("price_minor").map_err(db)?;
            let balance=lock_wallet(&mut tx,&command.actor).await?;
            if balance<price { return Err(AdError::Insufficient); }
            let book:Option<i64>=sqlx::query_scalar("SELECT b.id FROM books b
                JOIN rec_materializable_books visible ON visible.id=b.id
                WHERE b.id=$1 AND b.owner_user_id=$2 FOR SHARE OF b")
                .bind(command.purchase.book_id()).bind(command.actor.user_id)
                .fetch_optional(&mut *tx).await.map_err(db)?;
            if book.is_none() { return Err(AdError::Denied); }
            sqlx::query("SELECT user_id FROM user_level_grants WHERE user_id=$1 AND is_active FOR SHARE")
                .bind(command.actor.user_id).fetch_all(&mut *tx).await.map_err(db)?;
            // Recheck age after wallet/user locks; a stale form is no authority.
            let permitted:bool=sqlx::query_scalar("SELECT b.access_level='basic'
                OR (b.access_level='adult' AND (u.adult_enabled OR EXISTS(SELECT 1 FROM user_level_grants g
                    WHERE g.user_id=u.id AND g.level='pro_adult' AND g.is_active)))
                OR (b.access_level='pro_adult' AND EXISTS(SELECT 1 FROM user_level_grants g
                    WHERE g.user_id=u.id AND g.level='pro_adult' AND g.is_active))
                FROM books b JOIN users u ON u.id=$2 WHERE b.id=$1")
                .bind(command.purchase.book_id()).bind(command.actor.user_id).fetch_one(&mut *tx).await.map_err(db)?;
            if !permitted { return Err(AdError::Denied); }
            let campaign=match &command.purchase {
                AdPurchase::Daily(v)=>{
                    let valid:bool=sqlx::query_scalar("SELECT ends_at>clock_timestamp() FROM finance_ad_intents WHERE id=$1")
                        .bind(intent).fetch_one(&mut *tx).await.map_err(db)?;
                    if !valid { return Err(AdError::Expired); }
                    let overlap:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ad_legacy_campaigns c
                        WHERE c.book_id=$1 AND c.advertiser_id=$2 AND c.slot=$3 AND c.status IN('funded','paused')
                          AND c.starts_at<$5::text::timestamptz AND c.ends_at>$4::text::timestamptz)")
                        .bind(v.book_id).bind(command.actor.user_id).bind(v.placement.as_str())
                        .bind(&v.starts_at).bind(&v.ends_at).fetch_one(&mut *tx).await.map_err(db)?;
                    if overlap { return Err(AdError::Conflict); }
                    sqlx::query_scalar("INSERT INTO ad_legacy_campaigns(advertiser_id,book_id,offer_id,offer_revision,
                        slot,starts_at,ends_at,total_days,prepaid_days,rate_minor,price_minor,funding_intent_id,status)
                      SELECT advertiser_id,book_id,offer_id,offer_revision,slot,starts_at,ends_at,total_days,total_days,
                        rate_minor,price_minor,id,'funded' FROM finance_ad_intents WHERE id=$1 RETURNING id")
                        .bind(intent).fetch_one(&mut *tx).await.map_err(db)?
                },
                AdPurchase::Chapter(_)=>{
                    sqlx::query_scalar("INSERT INTO ad_chapter_campaigns(advertiser_id,promoted_book_id,package_id,
                        package_revision,funding_intent_id,status)
                      SELECT advertiser_id,book_id,offer_id,offer_revision,id,'funded' FROM finance_ad_intents
                      WHERE id=$1 RETURNING id").bind(intent).fetch_one(&mut *tx).await.map_err(db)?
                }
            };
            let after:i64=sqlx::query_scalar("UPDATE reader_wallet_accounts SET balance_minor=balance_minor-$2
                WHERE user_id=$1 AND currency='RUB' AND balance_minor>=$2 RETURNING balance_minor")
                .bind(command.actor.user_id).bind(price).fetch_optional(&mut *tx).await.map_err(db)?.ok_or(AdError::Insufficient)?;
            let reference=format!("ads:fund:{intent}");
            let wallet_entry:i64=sqlx::query_scalar("INSERT INTO reader_wallet_entries(user_id,kind,delta_minor,balance_after_minor,reference)
                VALUES($1,'ad_funding_debit',-$2,$3,$4) RETURNING id")
                .bind(command.actor.user_id).bind(price).bind(after).bind(&reference).fetch_one(&mut *tx).await.map_err(db)?;
            sqlx::query("UPDATE finance_ad_intents SET status='posted',campaign_id=$2 WHERE id=$1")
                .bind(intent).bind(campaign).execute(&mut *tx).await.map_err(db)?;
            sqlx::query("INSERT INTO finance_ad_funding(intent_id,advertiser_id,product,campaign_id,budget_minor,
                target_units,publisher_share_bps,wallet_entry_id)
              SELECT id,advertiser_id,product,$2,price_minor,target_units,publisher_share_bps,$3
              FROM finance_ad_intents WHERE id=$1").bind(intent).bind(campaign).bind(wallet_entry)
                .execute(&mut *tx).await.map_err(db)?;
            let posting=transaction(&mut tx,intent,&reference,"fund",price,Some(wallet_entry)).await?;
            entry(&mut tx,posting,"wallet",Some(command.actor.user_id),-price).await?;
            entry(&mut tx,posting,"escrow",None,price).await?;
            sqlx::query("INSERT INTO outbox_events(event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,payload,required_consumers)
                VALUES('AdCampaignPurchased',1,'ad_campaign',$1,$2,
                    jsonb_build_object('product',$3::TEXT,'campaign_id',$1::BIGINT,'intent_id',$4::BIGINT),ARRAY['ads_aggregate'])")
                .bind(campaign).bind(&reference).bind(command.purchase.product().as_str()).bind(intent)
                .execute(&mut *tx).await.map_err(db)?;
            Ok(AdPurchaseReceipt {campaign_id:AdCampaignId::new(campaign)?, product:command.purchase.product(),
                charged:MoneyMinor::new(price).map_err(|_|AdError::Invalid)?,
                balance_after:MoneyMinor::new(after).map_err(|_|AdError::Invalid)?,replayed:false})
        }.await;
        match result {
            Ok(receipt) => {
                tx.commit().await.map_err(db)?;
                Ok(receipt)
            }
            Err(e) => {
                tx.rollback().await.map_err(db)?;
                Err(e)
            }
        }
    }
}
