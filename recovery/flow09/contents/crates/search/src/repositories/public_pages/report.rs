use super::*;

impl PgPublicPages {
    /// Every source link and saved result is rechecked against current anonymous
    /// eligibility/revision in this RR snapshot; hidden targets are never rendered.
    pub async fn report(&self, origin: &str) -> Result<PublicPageReport, PageProbeError> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        limits(&mut tx, true).await?;
        let clock=sqlx::query("SELECT to_char(now() AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS measured,
            to_char(finished_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS checked,
            greatest(0,floor(extract(epoch FROM now()-finished_at)))::bigint AS age,
            finished_origin=$1 AS same_origin FROM flow_public_page_probe WHERE singleton")
            .bind(origin).fetch_one(&mut *tx).await.map_err(unavailable)?;
        let checked_at: Option<String> = clock.try_get("checked").map_err(unavailable)?;
        let age_seconds: Option<i64> = if checked_at.is_some() { clock.try_get("age").map_err(unavailable)? } else { None };
        let fresh = age_seconds.is_some_and(|age| age<=PROBE_FRESH_SECONDS)
            && clock.try_get::<Option<bool>,_>("same_origin").map_err(unavailable)?.unwrap_or(false);
        let live = candidates::read(&mut tx, false).await?;
        let stored=sqlx::query("SELECT *,to_char(last_success_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS last_success
            FROM flow_public_page_results ORDER BY slot LIMIT 4")
            .fetch_all(&mut *tx).await.map_err(unavailable)?;
        let mut rows = Vec::new();
        for candidate in live {
            let saved = stored.iter().find(|row| target(row).as_ref()==Ok(&candidate));
            let status = saved.filter(|_| fresh).map(|row| row.try_get::<Option<i16>,_>("http_status"))
                .transpose().map_err(unavailable)?.flatten().map(|s| s as u16);
            let baseline = saved.is_some_and(|row| row.try_get::<bool,_>("baseline_known").unwrap_or(false));
            let path = candidate.path();
            let state = if path.is_none() { PageState::NoTarget }
                else if status==Some(200) { PageState::Healthy }
                else if status.is_some() && baseline { PageState::Lost }
                else { PageState::Unknown };
            let last_success_at = saved.map(|row| row.try_get("last_success")).transpose().map_err(unavailable)?.flatten();
            rows.push(PublicPageRow { slot: candidate.slot, path, state, status, last_success_at });
        }
        let report = PublicPageReport {
            measured_at: clock.try_get("measured").map_err(unavailable)?, checked_at, age_seconds, rows,
        };
        tx.commit().await.map_err(unavailable)?;
        Ok(report)
    }
}
