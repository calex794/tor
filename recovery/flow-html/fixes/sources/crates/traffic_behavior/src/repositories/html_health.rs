//! Durable HTML-worker acknowledgements. No connection is retained across a queue
//! barrier; only a materialized immutable RIL batch can advance its checkpoint.
mod read;
use crate::domain::html_health::{
    HtmlCheckpoint, HtmlHealthError, HTML_CLOCK_MARGIN_MS, HTML_HEALTH_GAP_MS,
    HTML_HEALTH_LEASE_MS,
};
use sqlx::{PgPool, Postgres, Row, Transaction};

impl From<sqlx::Error> for HtmlHealthError {
    fn from(_: sqlx::Error) -> Self { Self::Unavailable }
}
#[derive(Clone)]
pub struct PgHtmlHealth { pool: PgPool }
impl PgHtmlHealth {
    pub fn new(pool: PgPool) -> Self { Self { pool } }
    /// A missing, unapplied capability is distinct from a broken installation.
    /// Run in the caller's snapshot so old counts cannot gain a newer proof.
    pub(crate) async fn capability_in(
        tx: &mut Transaction<'_, Postgres>,
    ) -> Result<bool, HtmlHealthError> {
        let row = sqlx::query(
            "SELECT to_regclass('ril_flow_health_epochs') IS NOT NULL AS epochs, \
             to_regclass('ril_flow_health_hourly') IS NOT NULL AS hourly, \
             to_regclass('_sqlx_migrations') IS NOT NULL AS migrations",
        )
        .fetch_one(&mut **tx)
        .await?;
        let epochs: bool = row.try_get("epochs")?;
        let hourly: bool = row.try_get("hourly")?;
        let migrations: bool = row.try_get("migrations")?;
        let applied: bool = if migrations {
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM _sqlx_migrations \
                 WHERE version=202611294685 AND success)",
            )
            .fetch_one(&mut **tx)
            .await?
        } else {
            false
        };
        match (epochs, hourly, applied) {
            (false, false, false) => Ok(false),
            (true, true, true) => Ok(true),
            _ => Err(HtmlHealthError::Unavailable),
        }
    }
    pub async fn ready(&self) -> Result<bool, HtmlHealthError> {
        self.ready_clock().await.map(|clock| clock.is_some())
    }
    /// DB clock sampled before this method returns. The service subtracts its
    /// monotonic age measured after the return to bound the preceding gap.
    pub(crate) async fn ready_clock(&self) -> Result<Option<i64>, HtmlHealthError> {
        let mut tx = self.pool.begin().await?;
        limits(&mut tx, true).await?;
        if !Self::capability_in(&mut tx).await? {
            tx.commit().await?;
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT to_regclass('ril_flow_observed_hourly') IS NOT NULL AS observed, \
             floor(extract(epoch FROM clock_timestamp())*1000)::bigint AS ms",
        )
        .fetch_one(&mut *tx)
        .await?;
        let clock = row.try_get::<bool, _>("observed")?
            .then_some(row.try_get("ms")?);
        tx.commit().await?;
        Ok(clock)
    }
    pub async fn start_epoch(&self) -> Result<(i64, i64), HtmlHealthError> {
        self.insert_epoch(None, 0, 0, 0, 0).await
    }
    /// Owner service port: preserve the unacknowledged interval before a new
    /// registration and all known counters not already acknowledged by a durable
    /// note. Both the closed gap and new live epoch commit in one transaction.
    pub async fn start_epoch_after_gap(
        &self,
        from_ms: i64,
        dropped: i64,
        failed: i64,
        recorded_dropped: i64,
        recorded_failed: i64,
    ) -> Result<(i64, i64), HtmlHealthError> {
        self.insert_epoch(
            Some(from_ms),
            dropped,
            failed,
            recorded_dropped,
            recorded_failed,
        )
        .await
    }
    async fn insert_epoch(
        &self,
        gap_from_ms: Option<i64>,
        dropped: i64,
        failed: i64,
        recorded_dropped: i64,
        recorded_failed: i64,
    ) -> Result<(i64, i64), HtmlHealthError> {
        if dropped < 0 || failed < 0 || recorded_dropped < 0 || recorded_failed < 0
            || recorded_dropped > dropped || recorded_failed > failed
            || gap_from_ms.is_some_and(|from| from <= 0)
        {
            return Err(HtmlHealthError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        limits(&mut tx, false).await?;
        let now: i64 = sqlx::query_scalar(
            "SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint",
        )
        .fetch_one(&mut *tx)
        .await?;
        if let Some(from) = gap_from_ms {
            if from > now {
                return Err(HtmlHealthError::Invalid);
            }
            let from = from.max(now - 181 * 86_400_000);
            let gap: i64 = sqlx::query_scalar(
                "INSERT INTO ril_flow_health_epochs \
                 (started_ms,confirmed_ms,lease_until_ms,retired,dropped,failed) \
                 VALUES($1,$1,$2,true,$3,$4) RETURNING epoch",
            )
            .bind(from)
            .bind(now)
            .bind(dropped)
            .bind(failed)
            .fetch_one(&mut *tx)
            .await?;
            flags(
                &mut tx,
                gap,
                from,
                now,
                dropped > recorded_dropped,
                failed > recorded_failed,
            )
            .await?;
        }
        let row = sqlx::query(
            "INSERT INTO ril_flow_health_epochs \
             (started_ms,confirmed_ms,lease_until_ms,dropped,failed) \
             VALUES($1,$1,$1+$2,$3,$4) RETURNING epoch,started_ms",
        )
        .bind(now)
        .bind(HTML_HEALTH_LEASE_MS)
        .bind(dropped)
        .bind(failed)
        .fetch_one(&mut *tx)
        .await?;
        let value = (row.try_get("epoch")?, row.try_get("started_ms")?);
        tx.commit().await?;
        Ok(value)
    }
    pub async fn begin(&self, epoch: i64) -> Result<HtmlCheckpoint, HtmlHealthError> {
        let mut tx = self.pool.begin().await?;
        limits(&mut tx, false).await?;
        let row = sqlx::query("WITH instant AS (SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint AS ms) UPDATE ril_flow_health_epochs e SET generation=generation+1,pending_ms=i.ms,lease_until_ms=i.ms+$2 FROM instant i WHERE epoch=$1 AND NOT retired AND lease_until_ms>=i.ms AND confirmed_ms>=i.ms-$3 RETURNING generation,pending_ms")
            .bind(epoch).bind(HTML_HEALTH_LEASE_MS).bind(HTML_HEALTH_GAP_MS)
            .fetch_optional(&mut *tx).await?.ok_or(HtmlHealthError::Expired)?;
        let point = HtmlCheckpoint { epoch, generation: row.try_get("generation")?, cut_ms: row.try_get("pending_ms")? };
        tx.commit().await?;
        Ok(point)
    }
    /// Renew a pending barrier without certifying any time. A lease which already
    /// expired cannot resume, even within the maximum checkpoint age.
    pub async fn renew(&self, point: HtmlCheckpoint) -> Result<i64, HtmlHealthError> {
        let mut tx = self.pool.begin().await?;
        limits(&mut tx, false).await?;
        let now = sqlx::query_scalar("WITH instant AS (SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint AS ms) UPDATE ril_flow_health_epochs e SET lease_until_ms=i.ms+$4 FROM instant i WHERE epoch=$1 AND generation=$2 AND pending_ms=$3 AND NOT retired AND lease_until_ms>=i.ms AND confirmed_ms>=i.ms-$5 RETURNING i.ms")
            .bind(point.epoch).bind(point.generation).bind(point.cut_ms)
            .bind(HTML_HEALTH_LEASE_MS).bind(HTML_HEALTH_GAP_MS)
            .fetch_optional(&mut *tx).await?.ok_or(HtmlHealthError::Expired)?;
        tx.commit().await?;
        Ok(now)
    }
    /// `None` is the ordered acknowledgement of an empty drain. For a real batch,
    /// done state and immutable receipt are checked in the same fenced transaction.
    pub async fn finish(&self, point: HtmlCheckpoint, batch: Option<&str>, dropped: i64, failed: i64) -> Result<bool, HtmlHealthError> {
        if point.epoch<=0 || point.generation<=0 || point.cut_ms<=0 || dropped<0 || failed<0 || batch.is_some_and(|id| id.len()!=36) {
            return Err(HtmlHealthError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        limits(&mut tx, false).await?;
        let row = sqlx::query("SELECT confirmed_ms,pending_ms,dropped,failed,floor(extract(epoch FROM clock_timestamp())*1000)::bigint AS now_ms FROM ril_flow_health_epochs WHERE epoch=$1 AND generation=$2 AND NOT retired AND lease_until_ms>=floor(extract(epoch FROM clock_timestamp())*1000)::bigint AND (pending_ms=$3 OR pending_ms IS NULL AND confirmed_ms>= $3-$5) AND $3-confirmed_ms BETWEEN 0 AND $4 AND $3>=floor(extract(epoch FROM clock_timestamp())*1000)::bigint-$4 FOR UPDATE")
            .bind(point.epoch).bind(point.generation).bind(point.cut_ms).bind(HTML_HEALTH_GAP_MS).bind(HTML_CLOCK_MARGIN_MS)
            .fetch_optional(&mut *tx).await?.ok_or(HtmlHealthError::Expired)?;
        let from:i64=row.try_get("confirmed_ms")?;
        if row.try_get::<Option<i64>,_>("pending_ms")?.is_none() {
            tx.commit().await?;
            return Ok(true);
        }
        let discarded = if let Some(id)=batch {
            let discarded: Option<i64>=sqlx::query_scalar("SELECT coalesce(r.discarded_events,0)::bigint FROM ril_flush_batches b LEFT JOIN ril_flow_observed_receipts r USING(batch_id) WHERE b.batch_id=$1 AND b.status='done'")
                .bind(id).fetch_optional(&mut *tx).await?;
            let Some(discarded)=discarded else {return Ok(false);};
            discarded>0
        } else {false};
        let old_dropped:i64=row.try_get("dropped")?;
        let old_failed:i64=row.try_get("failed")?;
        if dropped<old_dropped || failed<old_failed {return Err(HtmlHealthError::Invalid);}
        flags(&mut tx,point.epoch,from,row.try_get("now_ms")?,dropped>old_dropped || discarded,failed>old_failed).await?;
        // The request clock is checked against the leased DB clock. The margin
        // prevents a later accepted observation from entering an acknowledged hour.
        sqlx::query("UPDATE ril_flow_health_epochs SET confirmed_ms=greatest(confirmed_ms,$2-$5),pending_ms=NULL,dropped=$3,failed=$4 WHERE epoch=$1")
            .bind(point.epoch).bind(point.cut_ms).bind(dropped).bind(failed).bind(HTML_CLOCK_MARGIN_MS)
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }
    /// A returning or stopping worker permanently leaves its last unconfirmed
    /// tail unknown; another registered worker cannot conceal that interval.
    pub async fn retire(&self, epoch:i64, dropped:i64, failed:i64)->Result<(),HtmlHealthError>{
        let mut tx=self.pool.begin().await?;
        limits(&mut tx,false).await?;
        let row=sqlx::query("SELECT confirmed_ms,dropped,failed,floor(extract(epoch FROM clock_timestamp())*1000)::bigint AS now_ms FROM ril_flow_health_epochs WHERE epoch=$1 AND NOT retired FOR UPDATE")
            .bind(epoch).fetch_optional(&mut *tx).await?;
        if let Some(row)=row {
            flags(&mut tx,epoch,row.try_get("confirmed_ms")?,row.try_get("now_ms")?,dropped>row.try_get("dropped")?,failed>row.try_get("failed")?).await?;
            sqlx::query("UPDATE ril_flow_health_epochs SET retired=true,pending_ms=NULL,generation=generation+1,lease_until_ms=greatest(lease_until_ms,floor(extract(epoch FROM clock_timestamp())*1000)::bigint),dropped=greatest(dropped,$2),failed=greatest(failed,$3) WHERE epoch=$1")
                .bind(epoch).bind(dropped).bind(failed).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    /// Known loss is durable even while a batch is still pending. This does not
    /// advance confirmed time, refresh the lease, or declare a recovery.
    pub async fn note(&self,epoch:i64,dropped:i64,failed:i64)->Result<(),HtmlHealthError>{
        let mut tx=self.pool.begin().await?;
        limits(&mut tx,false).await?;
        let row=sqlx::query("SELECT confirmed_ms,dropped,failed,floor(extract(epoch FROM clock_timestamp())*1000)::bigint AS now_ms FROM ril_flow_health_epochs WHERE epoch=$1 AND NOT retired FOR UPDATE")
            .bind(epoch).fetch_optional(&mut *tx).await?.ok_or(HtmlHealthError::Expired)?;
        let old_dropped:i64=row.try_get("dropped")?;
        let old_failed:i64=row.try_get("failed")?;
        if dropped<old_dropped || failed<old_failed {return Err(HtmlHealthError::Invalid);}
        flags(&mut tx,epoch,row.try_get("confirmed_ms")?,row.try_get("now_ms")?,dropped>old_dropped,failed>old_failed).await?;
        sqlx::query("UPDATE ril_flow_health_epochs SET dropped=$2,failed=$3 WHERE epoch=$1")
            .bind(epoch).bind(dropped).bind(failed).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn prune(&self)->Result<(),HtmlHealthError>{
        let mut tx=self.pool.begin().await?;
        limits(&mut tx,false).await?;
        let before:i64=sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp()-interval '181 days')*1000)::bigint").fetch_one(&mut *tx).await?;
        sqlx::query("DELETE FROM ril_flow_health_hourly WHERE (epoch,bucket_ms) IN(SELECT epoch,bucket_ms FROM ril_flow_health_hourly WHERE bucket_ms<$1 ORDER BY bucket_ms,epoch LIMIT 500 FOR UPDATE SKIP LOCKED)").bind(before).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM ril_flow_health_epochs WHERE epoch IN(SELECT epoch FROM ril_flow_health_epochs e WHERE retired AND lease_until_ms<$1 AND NOT EXISTS(SELECT 1 FROM ril_flow_health_hourly h WHERE h.epoch=e.epoch) ORDER BY lease_until_ms,epoch LIMIT 100 FOR UPDATE SKIP LOCKED)").bind(before).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
}
async fn flags(tx:&mut Transaction<'_,Postgres>,epoch:i64,from:i64,through:i64,loss:bool,error:bool)->Result<(),HtmlHealthError>{
    if !loss && !error {return Ok(());}
    // At most 181 days, even when a paused process returns much later.
    let from=(from-HTML_CLOCK_MARGIN_MS).max(through-181*86_400_000);
    let first=from.div_euclid(3_600_000)*3_600_000;
    let last=(through+HTML_CLOCK_MARGIN_MS).div_euclid(3_600_000)*3_600_000;
    sqlx::query("INSERT INTO ril_flow_health_hourly(epoch,bucket_ms,had_loss,had_error) SELECT $1,b,$4,$5 FROM generate_series($2::bigint,$3::bigint,3600000) b ON CONFLICT(epoch,bucket_ms) DO UPDATE SET had_loss=ril_flow_health_hourly.had_loss OR EXCLUDED.had_loss,had_error=ril_flow_health_hourly.had_error OR EXCLUDED.had_error")
        .bind(epoch).bind(first).bind(last).bind(loss).bind(error).execute(&mut **tx).await?;
    Ok(())
}
async fn limits(tx:&mut Transaction<'_,Postgres>,read:bool)->Result<(),HtmlHealthError>{
    if read {sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY").execute(&mut **tx).await?;}
    sqlx::query("SELECT set_config('statement_timeout','750ms',true),set_config('lock_timeout','75ms',true)").execute(&mut **tx).await?;
    Ok(())
}
