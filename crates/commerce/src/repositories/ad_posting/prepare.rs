use super::*;
use crate::domain::money::IdempotencyKey;
use sha2::{Digest, Sha256};
use sqlx::Row;

pub(super) fn fingerprint(purchase: &AdPurchase) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"tor.ad.purchase.v1");
    h.update(purchase.product().as_str().as_bytes());
    let fields = match purchase {
        AdPurchase::Daily(v) => vec![
            v.book_id.to_string(),
            v.offer_id.to_string(),
            v.offer_revision.to_string(),
            v.placement.as_str().to_owned(),
            v.starts_at.clone(),
            v.ends_at.clone(),
            v.total_days.to_string(),
            v.prepaid_days.to_string(),
        ],
        AdPurchase::Chapter(v) => vec![
            v.book_id.to_string(),
            v.package_id.to_string(),
            v.expected_package_revision.to_string(),
        ],
    };
    for value in fields {
        h.update((value.len() as u64).to_be_bytes());
        h.update(value.as_bytes());
    }
    h.finalize().into()
}
impl PgAdPosting {
    pub(super) async fn prepare(&self, command: &AdPurchaseCommand) -> Result<i64, AdError> {
        IdempotencyKey::parse(&command.command_key).map_err(|_| AdError::Invalid)?;
        if command.actor.user_id <= 0 || command.purchase.book_id() <= 0 {
            return Err(AdError::Invalid);
        }
        let hash = fingerprint(&command.purchase);
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        let result=async {
            authorize(&mut tx,&command.actor).await?;
            if let Some(row)=sqlx::query("SELECT id,request_hash FROM finance_ad_intents
                WHERE advertiser_id=$1 AND command_key=$2").bind(command.actor.user_id)
                .bind(&command.command_key).fetch_optional(&mut *tx).await.map_err(db)? {
                if row.try_get::<Vec<u8>,_>("request_hash").map_err(db)?!=hash { return Err(AdError::IdempotencyConflict); }
                return row.try_get("id").map_err(db);
            }
            let eligible:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM books b
                JOIN rec_materializable_books visible ON visible.id=b.id JOIN users u ON u.id=$1
                WHERE b.id=$2 AND b.owner_user_id=$1 AND u.account_state='active'
                  AND (b.access_level='basic' OR (b.access_level='adult' AND (u.adult_enabled OR EXISTS(
                    SELECT 1 FROM user_level_grants g WHERE g.user_id=u.id AND g.level='pro_adult' AND g.is_active)))
                    OR (b.access_level='pro_adult' AND EXISTS(SELECT 1 FROM user_level_grants g
                        WHERE g.user_id=u.id AND g.level='pro_adult' AND g.is_active))))")
                .bind(command.actor.user_id).bind(command.purchase.book_id()).fetch_one(&mut *tx).await.map_err(db)?;
            if !eligible { return Err(AdError::Denied); }
            match &command.purchase {
                AdPurchase::Daily(v)=>{
                    if v.offer_id<=0 || v.offer_revision<=0 || v.total_days==0 || v.total_days>366
                        || v.prepaid_days!=v.total_days { return Err(AdError::Invalid); }
                    sqlx::query("INSERT INTO finance_ad_intents(advertiser_id,command_key,request_hash,product,book_id,
                        offer_id,offer_revision,slot,starts_at,ends_at,total_days,rate_minor,price_minor,target_units,
                        publisher_share_bps,hold_seconds,daily_cap,hourly_cap,session_cap,terms_version,cancellation_policy)
                      SELECT $1,$2,$3,'daily',$4,o.id,r.revision,r.slot,$7::text::timestamptz,$8::text::timestamptz,
                        $9,r.rate_minor,(r.rate_minor::NUMERIC*$9)::BIGINT,$9,0,0,r.daily_cap,r.hourly_cap,r.session_cap,r.terms_version,r.cancellation_policy
                      FROM ad_daily_offers o JOIN ad_daily_offer_revisions r ON r.offer_id=o.id AND r.revision=o.current_revision
                      WHERE o.id=$5 AND o.enabled AND r.revision=$6 AND r.slot=$10 AND $9 BETWEEN r.min_days AND r.max_days
                        AND $7::text::timestamptz>=date_trunc('day',clock_timestamp() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                        AND $8::text::timestamptz=$7::text::timestamptz+$9*INTERVAL '24 hours'
                        AND $7::text::timestamptz=date_trunc('day',$7::text::timestamptz AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                      ON CONFLICT(advertiser_id,command_key) DO NOTHING")
                        .bind(command.actor.user_id).bind(&command.command_key).bind(hash.as_slice()).bind(v.book_id)
                        .bind(v.offer_id).bind(v.offer_revision).bind(&v.starts_at).bind(&v.ends_at)
                        .bind(i32::try_from(v.total_days).map_err(|_|AdError::Invalid)?).bind(v.placement.as_str())
                        .execute(&mut *tx).await.map_err(db)?;
                },
                AdPurchase::Chapter(v)=>{
                    if v.package_id<=0 || v.expected_package_revision<=0 { return Err(AdError::Invalid); }
                    sqlx::query("INSERT INTO finance_ad_intents(advertiser_id,command_key,request_hash,product,book_id,
                        offer_id,offer_revision,slot,price_minor,target_units,publisher_share_bps,hold_seconds,
                        daily_cap,hourly_cap,session_cap,terms_version,cancellation_policy)
                      SELECT $1,$2,$3,'chapter',$4,p.id,r.revision,'chapter_end_free',r.price_minor,r.target_impressions,
                        r.publisher_share_bps,r.hold_seconds,r.daily_cap,r.hourly_cap,r.session_cap,r.terms_version,'unused_impressions'
                      FROM ad_packages p JOIN ad_package_revisions r ON r.package_id=p.id AND r.revision=p.current_revision
                      WHERE p.id=$5 AND p.enabled AND r.revision=$6
                      ON CONFLICT(advertiser_id,command_key) DO NOTHING")
                        .bind(command.actor.user_id).bind(&command.command_key).bind(hash.as_slice()).bind(v.book_id)
                        .bind(v.package_id).bind(v.expected_package_revision).execute(&mut *tx).await.map_err(db)?;
                }
            }
            let row=sqlx::query("SELECT id,request_hash FROM finance_ad_intents WHERE advertiser_id=$1 AND command_key=$2")
                .bind(command.actor.user_id).bind(&command.command_key).fetch_optional(&mut *tx).await.map_err(db)?
                .ok_or(AdError::Conflict)?;
            if row.try_get::<Vec<u8>,_>("request_hash").map_err(db)?!=hash { return Err(AdError::IdempotencyConflict); }
            row.try_get("id").map_err(db)
        }.await;
        match result {
            Ok(id) => {
                tx.commit().await.map_err(db)?;
                Ok(id)
            }
            Err(e) => {
                tx.rollback().await.map_err(db)?;
                Err(e)
            }
        }
    }
}
