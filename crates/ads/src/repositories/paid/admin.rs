use super::*;
use sqlx::Row;

impl PgPaidAds {
    pub async fn publish_daily(
        &self,
        a: &AdActor,
        spec: &PublishDaily,
    ) -> Result<(i64, i64), AdError> {
        slot(&spec.slot)?;
        if spec.rate_minor <= 0
            || spec.min_days <= 0
            || spec.max_days < spec.min_days
            || spec.max_days > 366
            || spec.terms_version.trim().is_empty()
            || spec.terms_version.chars().count() > 120
            || spec.cancellation_policy != "unused_full_utc_days"
            || spec.daily_cap <= 0
            || spec.hourly_cap <= 0
            || spec.session_cap <= 0
            || spec.hourly_cap > spec.daily_cap
            || spec.session_cap > spec.daily_cap
            || !(0..=3600).contains(&spec.min_interval_seconds)
        {
            return Err(AdError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        actor(&mut tx, a, true).await?;
        let (id, rev) = header(
            &mut tx,
            a,
            spec.id,
            spec.expected_revision,
            spec.enabled,
            false,
        )
        .await?;
        sqlx::query("INSERT INTO ad_daily_offer_revisions(offer_id,revision,slot,rate_minor,min_days,max_days,
    cancellation_policy,terms_version,published_by,daily_cap,hourly_cap,session_cap,min_interval_seconds) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)")
    .bind(id).bind(rev).bind(&spec.slot).bind(spec.rate_minor).bind(spec.min_days).bind(spec.max_days)
    .bind(&spec.cancellation_policy).bind(&spec.terms_version).bind(a.user_id)
    .bind(spec.daily_cap).bind(spec.hourly_cap).bind(spec.session_cap).bind(spec.min_interval_seconds).execute(&mut *tx).await.map_err(db)?;
        audit(&mut tx, a, "publish_daily", id, rev).await?;
        tx.commit().await.map_err(db)?;
        Ok((id, rev))
    }
    pub async fn publish_package(
        &self,
        a: &AdActor,
        spec: &PublishPackage,
    ) -> Result<(i64, i64), AdError> {
        if spec.price_minor <= 0
            || spec.target_impressions <= 0
            || !(0..=10000).contains(&spec.publisher_share_bps)
            || !(0..=31536000).contains(&spec.hold_seconds)
            || spec.daily_cap <= 0
            || spec.hourly_cap <= 0
            || spec.session_cap <= 0
            || spec.hourly_cap > spec.daily_cap
            || spec.session_cap > spec.daily_cap
            || spec.terms_version.trim().is_empty()
            || spec.terms_version.chars().count() > 120
        {
            return Err(AdError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        actor(&mut tx, a, true).await?;
        let (id, rev) = header(
            &mut tx,
            a,
            spec.id,
            spec.expected_revision,
            spec.enabled,
            true,
        )
        .await?;
        sqlx::query("INSERT INTO ad_package_revisions(package_id,revision,price_minor,price_micros_at_offer,target_impressions,
    publisher_share_bps,pricing_algorithm,hold_seconds,daily_cap,hourly_cap,session_cap,terms_version,published_by)
    VALUES($1,$2,$3,$3::NUMERIC*10000,$4,$5,'cumulative_minor_v1',$6,$7,$8,$9,$10,$11)")
    .bind(id).bind(rev).bind(spec.price_minor).bind(spec.target_impressions).bind(spec.publisher_share_bps)
    .bind(spec.hold_seconds).bind(spec.daily_cap).bind(spec.hourly_cap).bind(spec.session_cap).bind(&spec.terms_version)
    .bind(a.user_id).execute(&mut *tx).await.map_err(db)?;
        audit(&mut tx, a, "publish_package", id, rev).await?;
        tx.commit().await.map_err(db)?;
        Ok((id, rev))
    }
    pub async fn slot_policy(&self, a: &AdActor, spec: &SlotPolicy) -> Result<(), AdError> {
        let placement = slot(&spec.slot)?;
        if spec.expected_revision < 0
            || !(1..=4).contains(&spec.max_cards)
            || (spec.chapter_enabled
                && placement != tor_contracts::exposure::PromotionSlot::ChapterEndFree)
        {
            return Err(AdError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        actor(&mut tx, a, true).await?;
        let updated = if spec.expected_revision == 0 {
            sqlx::query("INSERT INTO ad_slot_policies(slot,paid_first,daily_enabled,chapter_enabled,max_cards,revision,updated_by)
    VALUES($1,$2,$3,$4,$5,1,$6) ON CONFLICT(slot) DO NOTHING")
    .bind(&spec.slot).bind(spec.paid_first).bind(spec.daily_enabled).bind(spec.chapter_enabled).bind(spec.max_cards)
    .bind(a.user_id).execute(&mut *tx).await.map_err(db)?.rows_affected()
        } else {
            sqlx::query("UPDATE ad_slot_policies SET paid_first=$2,daily_enabled=$3,chapter_enabled=$4,max_cards=$5,
    revision=revision+1,updated_by=$6,updated_at=clock_timestamp() WHERE slot=$1 AND revision=$7")
    .bind(&spec.slot).bind(spec.paid_first).bind(spec.daily_enabled).bind(spec.chapter_enabled).bind(spec.max_cards)
    .bind(a.user_id).bind(spec.expected_revision).execute(&mut *tx).await.map_err(db)?.rows_affected()
        };
        if updated != 1 {
            return Err(AdError::Conflict);
        }
        audit(
            &mut tx,
            a,
            &format!("slot_policy:{}", spec.slot),
            0,
            spec.expected_revision + 1,
        )
        .await?;
        tx.commit().await.map_err(db)
    }
    pub async fn policies(
        &self,
        a: &AdActor,
    ) -> Result<Vec<(String, i64, bool, bool, bool, i32)>, AdError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        actor(&mut tx, a, true).await?;
        let rows=sqlx::query("SELECT slot,revision,paid_first,daily_enabled,chapter_enabled,max_cards FROM ad_slot_policies ORDER BY slot LIMIT 3")
    .fetch_all(&mut *tx).await.map_err(db)?;
        let result = rows
            .iter()
            .map(|r| {
                Ok((
                    r.try_get("slot").map_err(db)?,
                    r.try_get("revision").map_err(db)?,
                    r.try_get("paid_first").map_err(db)?,
                    r.try_get("daily_enabled").map_err(db)?,
                    r.try_get("chapter_enabled").map_err(db)?,
                    r.try_get("max_cards").map_err(db)?,
                ))
            })
            .collect();
        tx.commit().await.map_err(db)?;
        result
    }
}
async fn header(
    tx: &mut Transaction<'_, Postgres>,
    a: &AdActor,
    id: Option<i64>,
    expected: i64,
    enabled: bool,
    package: bool,
) -> Result<(i64, i64), AdError> {
    if expected < 0
        || id.is_some_and(|v| v <= 0)
        || (id.is_none() && expected != 0)
        || (id.is_some() && expected == 0)
    {
        return Err(AdError::Invalid);
    }
    let next = expected.checked_add(1).ok_or(AdError::Invalid)?;
    let id = if let Some(id) = id {
        let query = if package {
            "UPDATE ad_packages SET current_revision=$2,enabled=$3 WHERE id=$1 AND current_revision=$4 RETURNING id"
        } else {
            "UPDATE ad_daily_offers SET current_revision=$2,enabled=$3 WHERE id=$1 AND current_revision=$4 RETURNING id"
        };
        sqlx::query_scalar(query)
            .bind(id)
            .bind(next)
            .bind(enabled)
            .bind(expected)
            .fetch_optional(&mut **tx)
            .await
            .map_err(db)?
            .ok_or(AdError::Conflict)?
    } else {
        let query = if package {
            "INSERT INTO ad_packages(current_revision,enabled,created_by) VALUES(1,$1,$2) RETURNING id"
        } else {
            "INSERT INTO ad_daily_offers(current_revision,enabled,created_by) VALUES(1,$1,$2) RETURNING id"
        };
        sqlx::query_scalar(query)
            .bind(enabled)
            .bind(a.user_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(db)?
    };
    Ok((id, next))
}
async fn audit(
    tx: &mut Transaction<'_, Postgres>,
    a: &AdActor,
    operation: &str,
    id: i64,
    revision: i64,
) -> Result<(), AdError> {
    sqlx::query("INSERT INTO ad_administration_audit(actor_id,operation,resource_id,revision) VALUES($1,$2,$3,$4)")
  .bind(a.user_id).bind(operation).bind(id).bind(revision).execute(&mut **tx).await.map_err(db)?;
    Ok(())
}
