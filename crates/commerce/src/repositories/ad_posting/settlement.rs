use super::*;
use sqlx::Row;

pub(super) async fn chapter(
    tx: &mut Transaction<'_, Postgres>,
    campaign: i64,
    limit: u32,
) -> Result<u32, AdError> {
    let intent: Option<i64> = sqlx::query_scalar(
        "SELECT intent_id FROM finance_ad_funding
        WHERE product='chapter' AND campaign_id=$1 FOR NO KEY UPDATE",
    )
    .bind(campaign)
    .fetch_optional(&mut **tx)
    .await
    .map_err(db)?;
    let intent = intent.ok_or(AdError::NotFound)?;
    let rows = sqlx::query(
        "SELECT r.*,i.hold_seconds FROM ad_impression_reservations r
        JOIN finance_ad_intents i ON i.id=r.intent_id
        WHERE r.intent_id=$1 AND r.settled_at IS NULL ORDER BY r.ordinal LIMIT $2 FOR UPDATE OF r",
    )
    .bind(intent)
    .bind(i64::from(limit))
    .fetch_all(&mut **tx)
    .await
    .map_err(db)?;
    let mut publishers = rows
        .iter()
        .filter_map(|r| r.try_get::<i64, _>("publisher_id").ok())
        .collect::<Vec<_>>();
    publishers.sort_unstable();
    publishers.dedup();
    // All campaign workers acquire shared publisher balances in one order.
    for publisher in &publishers {
        sqlx::query("INSERT INTO finance_ad_publisher_balances(user_id) VALUES($1) ON CONFLICT(user_id) DO NOTHING")
            .bind(publisher).execute(&mut **tx).await.map_err(db)?;
    }
    sqlx::query("SELECT user_id FROM finance_ad_publisher_balances WHERE user_id=ANY($1::BIGINT[]) ORDER BY user_id FOR UPDATE")
        .bind(&publishers).fetch_all(&mut **tx).await.map_err(db)?;
    for row in &rows {
        let id: i64 = row.try_get("id").map_err(db)?;
        let ordinal: i64 = row.try_get("ordinal").map_err(db)?;
        let gross: i64 = row.try_get("gross_minor").map_err(db)?;
        let publisher: i64 = row.try_get("publisher_id").map_err(db)?;
        let share: i64 = row.try_get("publisher_minor").map_err(db)?;
        let platform: i64 = row.try_get("platform_minor").map_err(db)?;
        let posting = transaction(
            tx,
            intent,
            &format!("ads:settle:{intent}:{ordinal}"),
            "settle",
            gross,
            None,
        )
        .await?;
        entry(tx, posting, "reserved", None, -gross).await?;
        entry(tx, posting, "publisher_pending", Some(publisher), share).await?;
        entry(tx, posting, "platform", None, platform).await?;
        if share > 0 {
            sqlx::query("INSERT INTO finance_ad_publisher_balances(user_id,pending_minor) VALUES($1,$2)
                ON CONFLICT(user_id) DO UPDATE SET pending_minor=finance_ad_publisher_balances.pending_minor+$2")
                .bind(publisher).bind(share).execute(&mut **tx).await.map_err(db)?;
            sqlx::query("INSERT INTO finance_ad_publisher_credits(intent_id,publisher_id,settlement_transaction_id,amount_minor,clear_at)
                VALUES($1,$2,$3,$4,(SELECT created_at FROM finance_ad_transactions WHERE id=$3)+$5*interval '1 second')")
                .bind(intent).bind(publisher).bind(posting).bind(share)
                .bind(row.try_get::<i32,_>("hold_seconds").map_err(db)?).execute(&mut **tx).await.map_err(db)?;
        }
        sqlx::query("INSERT INTO ad_settlements(reservation_id,transaction_id) VALUES($1,$2)")
            .bind(id)
            .bind(posting)
            .execute(&mut **tx)
            .await
            .map_err(db)?;
        sqlx::query("UPDATE ad_impression_reservations SET settlement_transaction_id=$2,settled_at=(SELECT created_at FROM finance_ad_transactions WHERE id=$2) WHERE id=$1")
            .bind(id).bind(posting).execute(&mut **tx).await.map_err(db)?;
        sqlx::query(
            "UPDATE finance_ad_funding SET settled_minor=settled_minor+$2,
            publisher_settled_minor=publisher_settled_minor+$3 WHERE intent_id=$1",
        )
        .bind(intent)
        .bind(gross)
        .bind(share)
        .execute(&mut **tx)
        .await
        .map_err(db)?;
        sqlx::query("UPDATE ad_campaign_stats SET settled=settled+1,spent_minor=spent_minor+$2,
            publisher_minor=publisher_minor+$3,platform_minor=platform_minor+$4 WHERE campaign_id=$1")
            .bind(campaign).bind(gross).bind(share).bind(platform).execute(&mut **tx).await.map_err(db)?;
    }
    sqlx::query("UPDATE ad_chapter_campaigns c SET status='completed',revision=revision+1
        FROM finance_ad_funding f WHERE f.intent_id=c.funding_intent_id AND c.id=$1
          AND c.status='funded' AND f.reserved_units=f.target_units AND NOT EXISTS(
            SELECT 1 FROM ad_impression_reservations r WHERE r.intent_id=f.intent_id AND r.settled_at IS NULL)")
        .bind(campaign).execute(&mut **tx).await.map_err(db)?;
    u32::try_from(rows.len()).map_err(|_| AdError::Invalid)
}

pub(super) async fn daily(
    tx: &mut Transaction<'_, Postgres>,
    intent: i64,
    cancel: bool,
) -> Result<(), AdError> {
    let row = sqlx::query(
        "SELECT f.*,i.rate_minor,LEAST(i.total_days,GREATEST(0,
        CASE WHEN $2 THEN ceil(extract(epoch FROM clock_timestamp()-i.starts_at)/86400)
          ELSE floor(extract(epoch FROM clock_timestamp()-i.starts_at)/86400) END))::BIGINT AS due
        FROM finance_ad_funding f JOIN finance_ad_intents i ON i.id=f.intent_id
        WHERE f.intent_id=$1 AND f.product='daily' FOR NO KEY UPDATE OF f",
    )
    .bind(intent)
    .bind(cancel)
    .fetch_one(&mut **tx)
    .await
    .map_err(db)?;
    let due: i64 = row.try_get("due").map_err(db)?;
    let previous: i64 = row.try_get("reserved_units").map_err(db)?;
    let rate: i64 = row.try_get("rate_minor").map_err(db)?;
    for ordinal in previous + 1..=due {
        let reserve = transaction(
            tx,
            intent,
            &format!("ads:day-reserve:{intent}:{ordinal}"),
            "reserve",
            rate,
            None,
        )
        .await?;
        entry(tx, reserve, "escrow", None, -rate).await?;
        entry(tx, reserve, "reserved", None, rate).await?;
        let settle = transaction(
            tx,
            intent,
            &format!("ads:day-settle:{intent}:{ordinal}"),
            "settle",
            rate,
            None,
        )
        .await?;
        entry(tx, settle, "reserved", None, -rate).await?;
        entry(tx, settle, "platform", None, rate).await?;
        sqlx::query("INSERT INTO finance_ad_daily_units(intent_id,ordinal,reservation_transaction_id,settlement_transaction_id)
            VALUES($1,$2,$3,$4)").bind(intent).bind(ordinal).bind(reserve).bind(settle)
            .execute(&mut **tx).await.map_err(db)?;
    }
    if due > previous {
        sqlx::query("UPDATE finance_ad_funding SET reserved_units=$2,reserved_minor=$3,settled_minor=$3 WHERE intent_id=$1")
            .bind(intent).bind(due).bind(due.checked_mul(rate).ok_or(AdError::Invalid)?)
            .execute(&mut **tx).await.map_err(db)?;
    }
    Ok(())
}
