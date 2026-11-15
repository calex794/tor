use super::*;
use sha2::{Digest, Sha256};
use sqlx::Row;

pub(super) async fn allow(
    tx: &mut Transaction<'_, Postgres>,
    a: &AdActor,
    row: &sqlx::postgres::PgRow,
) -> Result<bool, AdError> {
    let campaign: i64 = row.try_get("id").map_err(db)?;
    let mut h = Sha256::new();
    h.update(b"tor.ad.daily.frequency.v1");
    h.update(campaign.to_be_bytes());
    h.update(a.user_id.to_be_bytes());
    let user: [u8; 32] = h.finalize().into();
    let mut h = Sha256::new();
    h.update(b"tor.ad.daily.session.v1");
    h.update(campaign.to_be_bytes());
    h.update(a.session_digest);
    let session: [u8; 32] = h.finalize().into();
    sqlx::query("SAVEPOINT ad_daily_cap")
        .execute(&mut **tx)
        .await
        .map_err(db)?;
    for (kind, subject, cap) in [
        (
            "day",
            user.as_slice(),
            row.try_get::<i32, _>("daily_cap").map_err(db)?,
        ),
        (
            "hour",
            user.as_slice(),
            row.try_get::<i32, _>("hourly_cap").map_err(db)?,
        ),
        (
            "session",
            session.as_slice(),
            row.try_get::<i32, _>("session_cap").map_err(db)?,
        ),
    ] {
        let accepted:Option<i32>=sqlx::query_scalar("INSERT INTO ad_daily_frequency_buckets
    (campaign_id,subject_hash,bucket,window_start,count,expires_at) VALUES($1,$2,$3,
      CASE $3 WHEN 'day' THEN date_trunc('day',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
        WHEN 'hour' THEN date_trunc('hour',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC' ELSE 'epoch'::TIMESTAMPTZ END,1,
      CASE $3 WHEN 'session' THEN (SELECT expires_at FROM sessions WHERE token_hash=$6) ELSE now()+interval '2 days' END)
    ON CONFLICT(campaign_id,subject_hash,bucket,window_start) DO UPDATE SET count=ad_daily_frequency_buckets.count+1,
      last_served_at=clock_timestamp() WHERE ad_daily_frequency_buckets.count<$4
      AND ($3<>'session' OR ad_daily_frequency_buckets.last_served_at<=clock_timestamp()-$5*interval '1 second')
    RETURNING count")
    .bind(campaign).bind(subject).bind(kind).bind(cap).bind(row.try_get::<i32,_>("min_interval_seconds").map_err(db)?)
    .bind(a.session_digest.as_slice()).fetch_optional(&mut **tx).await.map_err(db)?;
        if accepted.is_none() {
            sqlx::query("ROLLBACK TO SAVEPOINT ad_daily_cap")
                .execute(&mut **tx)
                .await
                .map_err(db)?;
            sqlx::query("RELEASE SAVEPOINT ad_daily_cap")
                .execute(&mut **tx)
                .await
                .map_err(db)?;
            return Ok(false);
        }
    }
    sqlx::query("RELEASE SAVEPOINT ad_daily_cap")
        .execute(&mut **tx)
        .await
        .map_err(db)?;
    Ok(true)
}
