//! F-owned durable source/day snapshots and fenced jobs; never joins a reader table.
mod report;
use crate::domain::seo_analytics::*;
use crate::services::seo_analytics::SeoAnalyticsStore;
use async_trait::async_trait;
use sqlx::{PgPool, Postgres, Row, Transaction};
type E = SeoAnalyticsError;
#[derive(Clone)]
pub struct PgSeoAnalytics {
    pool: PgPool,
}
impl PgSeoAnalytics {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    /// Called only by the explicitly enabled operator-configured worker.
    pub async fn configure(&self, key: SourceKey) -> Result<Source, E> {
        key.validate()?;
        let mut tx = self.pool.begin().await.map_err(|_| E::Unavailable)?;
        limits(&mut tx, false).await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,71203))")
            .bind(format!("flow-seo:{}", key.provider.key()))
            .execute(&mut *tx)
            .await
            .map_err(|_| E::Unavailable)?;
        sqlx::query("UPDATE flow_seo_sources SET enabled=false WHERE provider=$1 AND enabled AND (property<>$2 OR origin<>$3)")
            .bind(key.provider.key()).bind(&key.property).bind(&key.origin).execute(&mut *tx).await.map_err(|_|E::Unavailable)?;
        let id=sqlx::query_scalar("INSERT INTO flow_seo_sources(provider,property,origin) VALUES($1,$2,$3) ON CONFLICT(provider,property,origin) DO UPDATE SET enabled=true RETURNING id")
            .bind(key.provider.key()).bind(&key.property).bind(&key.origin).fetch_one(&mut *tx).await.map_err(|_|E::Unavailable)?;
        tx.commit().await.map_err(|_| E::Unavailable)?;
        Ok(Source { id, key })
    }
    /// Refresh only seven recent provider-labelled dates. A request coalesces with
    /// a running job; successful refreshes are not requeued within fifteen minutes.
    pub async fn request_refresh(&self, provider: Provider) -> Result<u64, E> {
        let mut tx = self.pool.begin().await.map_err(|_| E::Unavailable)?;
        limits(&mut tx, false).await?;
        let result=sqlx::query("INSERT INTO flow_seo_jobs(source_id,day)
            SELECT s.id,((clock_timestamp() AT TIME ZONE 'UTC')::date-n)::date
            FROM flow_seo_sources s CROSS JOIN generate_series(3,9) n
            WHERE s.provider=$1 AND s.enabled
            ON CONFLICT(source_id,day) DO UPDATE SET state='pending',lease_until=NULL,next_attempt_at=now()
            WHERE flow_seo_jobs.state<>'running' AND (flow_seo_jobs.last_attempt_at IS NULL OR flow_seo_jobs.last_attempt_at<now()-interval '15 minutes')")
            .bind(provider.key()).execute(&mut *tx).await.map_err(|_|E::Unavailable)?;
        tx.commit().await.map_err(|_| E::Unavailable)?;
        Ok(result.rows_affected())
    }
    /// Initial history is queued once per source/date; completed dates are not reset.
    /// Yandex URL analytics only exposes a recent window; older dates stay unknown.
    pub async fn request_backfill(&self, source: &Source) -> Result<u64, E> {
        source.key.validate()?;
        let mut tx = self.pool.begin().await.map_err(|_| E::Unavailable)?;
        limits(&mut tx, false).await?;
        let result=sqlx::query("INSERT INTO flow_seo_jobs(source_id,day)
            SELECT s.id,(now() AT TIME ZONE 'UTC')::date-n FROM flow_seo_sources s
            CROSS JOIN LATERAL generate_series(3,CASE WHEN s.provider='google' THEN 182 ELSE 13 END) n
            WHERE s.id=$1 AND s.enabled AND s.property=$2 AND s.origin=$3
            ON CONFLICT(source_id,day) DO NOTHING")
            .bind(source.id).bind(&source.key.property).bind(&source.key.origin)
            .execute(&mut *tx).await.map_err(|_|E::Unavailable)?;
        tx.commit().await.map_err(|_| E::Unavailable)?;
        Ok(result.rows_affected())
    }
    /// One old source/day bounds cascading deletion to the import cap.
    pub async fn prune_one(&self) -> Result<(), E> {
        let mut tx = self.pool.begin().await.map_err(|_| E::Unavailable)?;
        limits(&mut tx, false).await?;
        sqlx::query("DELETE FROM flow_seo_daily WHERE (source_id,day) IN (SELECT source_id,day FROM flow_seo_daily WHERE day<(now() AT TIME ZONE 'UTC')::date-200 ORDER BY day LIMIT 1 FOR UPDATE SKIP LOCKED)")
            .execute(&mut *tx).await.map_err(|_|E::Unavailable)?;
        sqlx::query("DELETE FROM flow_seo_jobs WHERE id IN (SELECT id FROM flow_seo_jobs WHERE day<(now() AT TIME ZONE 'UTC')::date-200 AND (state<>'running' OR lease_until<clock_timestamp()) ORDER BY day LIMIT 100 FOR UPDATE SKIP LOCKED)")
            .execute(&mut *tx).await.map_err(|_|E::Unavailable)?;
        tx.commit().await.map_err(|_| E::Unavailable)
    }
}
async fn limits(tx: &mut Transaction<'_, Postgres>, read: bool) -> Result<(), E> {
    if read {
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut **tx)
            .await
            .map_err(|_| E::Unavailable)?;
        sqlx::query("SET LOCAL statement_timeout='750ms'")
            .execute(&mut **tx)
            .await
            .map_err(|_| E::Unavailable)?;
    } else {
        sqlx::query("SET LOCAL statement_timeout='3000ms'")
            .execute(&mut **tx)
            .await
            .map_err(|_| E::Unavailable)?;
    }
    sqlx::query("SET LOCAL lock_timeout='100ms'")
        .execute(&mut **tx)
        .await
        .map_err(|_| E::Unavailable)?;
    Ok(())
}
#[async_trait]
impl SeoAnalyticsStore for PgSeoAnalytics {
    async fn claim(&self, source: &Source) -> Result<Option<Job>, E> {
        source.key.validate()?;
        let mut tx = self.pool.begin().await.map_err(|_| E::Unavailable)?;
        limits(&mut tx, false).await?;
        let row=sqlx::query("WITH candidate AS (
            SELECT j.id FROM flow_seo_jobs j JOIN flow_seo_sources s ON s.id=j.source_id
            WHERE s.id=$1 AND s.enabled AND s.provider=$2 AND s.property=$3 AND s.origin=$4
              AND j.next_attempt_at<=now() AND j.day BETWEEN (now() AT TIME ZONE 'UTC')::date-(CASE WHEN s.provider='google' THEN 182 ELSE 13 END) AND (now() AT TIME ZONE 'UTC')::date-1
              AND (j.state IN ('pending','error') OR (j.state='running' AND j.lease_until<now()))
            ORDER BY j.day DESC LIMIT 1 FOR UPDATE OF j SKIP LOCKED)
          UPDATE flow_seo_jobs j SET state='running',epoch=epoch+1,attempts=least(attempts+1,1000000),
            lease_until=clock_timestamp()+interval '180 seconds',last_attempt_at=clock_timestamp()
          FROM candidate c WHERE j.id=c.id RETURNING j.id,j.epoch,j.day::text")
            .bind(source.id).bind(source.key.provider.key()).bind(&source.key.property).bind(&source.key.origin)
            .fetch_optional(&mut *tx).await.map_err(|_|E::Unavailable)?;
        tx.commit().await.map_err(|_| E::Unavailable)?;
        row.map(|r| {
            Ok(Job {
                id: r.try_get("id").map_err(|_| E::Unavailable)?,
                epoch: r.try_get("epoch").map_err(|_| E::Unavailable)?,
                day: r.try_get("day").map_err(|_| E::Unavailable)?,
                source: source.clone(),
            })
        })
        .transpose()
    }
    async fn finish(&self, job: &Job, snapshot: &Snapshot) -> Result<(), E> {
        if !valid_day(&job.day) {
            return Err(E::Invalid);
        }
        snapshot.validate(&job.source.key.origin)?;
        let mut tx = self.pool.begin().await.map_err(|_| E::Unavailable)?;
        limits(&mut tx, false).await?;
        // Serialize the enabled-source check with a configuration replacement.
        let enabled: Option<i64> = sqlx::query_scalar("SELECT id FROM flow_seo_sources WHERE id=$1 AND enabled AND provider=$2 AND property=$3 AND origin=$4 FOR SHARE")
            .bind(job.source.id).bind(job.source.key.provider.key()).bind(&job.source.key.property).bind(&job.source.key.origin)
            .fetch_optional(&mut *tx).await.map_err(|_| E::Unavailable)?;
        if enabled.is_none() {
            return Err(E::Stale);
        }
        let changed=sqlx::query("UPDATE flow_seo_jobs j SET state='done',lease_until=NULL,last_error=NULL
            WHERE j.id=$1 AND j.source_id=$2 AND j.day=$3::text::date AND j.epoch=$4 AND j.state='running'
              AND j.lease_until>clock_timestamp() AND EXISTS(SELECT 1 FROM flow_seo_sources s WHERE s.id=j.source_id AND s.enabled)")
            .bind(job.id).bind(job.source.id).bind(&job.day).bind(job.epoch).execute(&mut *tx).await.map_err(|_|E::Unavailable)?;
        if changed.rows_affected() != 1 {
            return Err(E::Stale);
        }
        let totals = snapshot.totals.as_ref();
        sqlx::query("INSERT INTO flow_seo_daily(source_id,day,status,total_clicks,total_impressions,total_position,provider_rows,omitted_rows,limit_reached)
            VALUES($1,$2::text::date,$3,$4,$5,$6,$7,$8,$9)
            ON CONFLICT(source_id,day) DO UPDATE SET measured_at=clock_timestamp(),status=excluded.status,
            total_clicks=excluded.total_clicks,total_impressions=excluded.total_impressions,total_position=excluded.total_position,
            provider_rows=excluded.provider_rows,omitted_rows=excluded.omitted_rows,limit_reached=excluded.limit_reached")
            .bind(job.source.id).bind(&job.day).bind(if snapshot.available(){"available"}else{"no_data"})
            .bind(totals.map(|m|m.clicks as i64)).bind(totals.map(|m|m.impressions as i64)).bind(totals.and_then(|m|m.position))
            .bind(snapshot.provider_rows as i64).bind(snapshot.omitted_rows as i64).bind(snapshot.limit_reached)
            .execute(&mut *tx).await.map_err(|_|E::Unavailable)?;
        sqlx::query("DELETE FROM flow_seo_pages WHERE source_id=$1 AND day=$2::text::date")
            .bind(job.source.id)
            .bind(&job.day)
            .execute(&mut *tx)
            .await
            .map_err(|_| E::Unavailable)?;
        let rows:Vec<_>=snapshot.pages.iter().map(|p|serde_json::json!({"path":p.path,"clicks":p.metrics.clicks,"impressions":p.metrics.impressions,"position":p.metrics.position})).collect();
        sqlx::query("INSERT INTO flow_seo_pages(source_id,day,path,clicks,impressions,position)
            SELECT $1,$2::text::date,p.path,p.clicks,p.impressions,p.position
            FROM jsonb_to_recordset($3::text::jsonb) p(path text,clicks bigint,impressions bigint,position double precision)")
            .bind(job.source.id).bind(&job.day).bind(serde_json::to_string(&rows).map_err(|_|E::InvalidResponse)?)
            .execute(&mut *tx).await.map_err(|_|E::Unavailable)?;
        tx.commit().await.map_err(|_| E::Unavailable)
    }
    async fn fail(&self, job: &Job, error: E) -> Result<(), E> {
        let mut tx = self.pool.begin().await.map_err(|_| E::Unavailable)?;
        limits(&mut tx, false).await?;
        let changed=sqlx::query("UPDATE flow_seo_jobs SET state='error',lease_until=NULL,last_error=$5,
            next_attempt_at=clock_timestamp()+make_interval(secs=>least(3600::double precision,60*power(2,least(attempts,6))))
            WHERE id=$1 AND source_id=$2 AND day=$3::text::date AND epoch=$4 AND state='running' AND lease_until>clock_timestamp()")
            .bind(job.id).bind(job.source.id).bind(&job.day).bind(job.epoch).bind(error.code())
            .execute(&mut *tx).await.map_err(|_|E::Unavailable)?;
        if changed.rows_affected() != 1 {
            return Err(E::Stale);
        }
        tx.commit().await.map_err(|_| E::Unavailable)
    }
}
