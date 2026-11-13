use super::*;
impl PgSeoAnalytics {
    pub async fn report(
        &self,
        days: i32,
        provider: Provider,
        after: Option<&str>,
    ) -> Result<SeoReport, E> {
        if !matches!(days, 1 | 7 | 30 | 90)
            || after.is_some_and(|s| {
                s.len() > 64
                    || !(s == "/"
                        || s == "/catalog"
                        || super::super::super::domain::seo_analytics::canonical_path(
                            "https://example.com",
                            &format!("https://example.com{s}"),
                        )
                        .is_some())
            })
        {
            return Err(E::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(|_| E::Unavailable)?;
        limits(&mut tx, true).await?;
        let (anchor,measured_at):(String,String)=sqlx::query_as("SELECT ((now() AT TIME ZONE 'UTC')::date-2)::text,to_char(now() AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')")
            .fetch_one(&mut *tx).await.map_err(|_|E::Unavailable)?;
        let rows=sqlx::query("SELECT s.id,s.provider,s.property,s.origin,
            (SELECT to_char(max(measured_at) AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') FROM flow_seo_daily d WHERE d.source_id=s.id AND d.status='available') AS last_success,
            (SELECT last_error FROM flow_seo_jobs j WHERE j.source_id=s.id AND j.state='error' ORDER BY last_attempt_at DESC NULLS LAST,id DESC LIMIT 1) AS last_error,
            (SELECT count(*) FROM flow_seo_jobs j WHERE j.source_id=s.id AND j.state IN('pending','error')) AS pending,
            (SELECT count(*) FROM flow_seo_jobs j WHERE j.source_id=s.id AND j.state='running') AS running
            FROM flow_seo_sources s WHERE s.enabled ORDER BY s.provider,s.id LIMIT 2")
            .fetch_all(&mut *tx).await.map_err(|_|E::Unavailable)?;
        let mut sources = Vec::new();
        for row in rows {
            sources.push(SourceStatus {
                source: Source {
                    id: row.try_get("id").map_err(|_| E::Unavailable)?,
                    key: SourceKey {
                        provider: Provider::parse(
                            row.try_get("provider").map_err(|_| E::Unavailable)?,
                        )
                        .ok_or(E::Unavailable)?,
                        property: row.try_get("property").map_err(|_| E::Unavailable)?,
                        origin: row.try_get("origin").map_err(|_| E::Unavailable)?,
                    },
                },
                last_success: row.try_get("last_success").map_err(|_| E::Unavailable)?,
                last_error: row.try_get("last_error").map_err(|_| E::Unavailable)?,
                pending: row.try_get("pending").map_err(|_| E::Unavailable)?,
                running: row.try_get("running").map_err(|_| E::Unavailable)?,
            });
        }
        let id = sources
            .iter()
            .find(|s| s.source.key.provider == provider)
            .map(|s| s.source.id)
            .unwrap_or(0);
        let current = period(&mut tx, id, &anchor, days, 0).await?;
        let previous = period(&mut tx, id, &anchor, days, 1).await?;
        // The source may contain 50k rows and many no-longer-public paths.
        // Scan a bounded index-ordered batch before consulting public
        // projections; a hidden prefix cannot truncate the visible page.
        const SCAN_BATCH: usize = 256; // matches LIMIT in pages.sql
        let mut cursor = after.unwrap_or("").to_owned();
        let mut pages = Vec::new();
        loop {
            let rows = sqlx::query(include_str!("pages.sql"))
                .bind(id)
                .bind(&current.from)
                .bind(&current.until)
                .bind(&cursor)
                .fetch_all(&mut *tx)
                .await
                .map_err(|_| E::Unavailable)?;
            let scanned = rows.len();
            if scanned == 0 {
                break;
            }
            for r in rows {
                let path: String = r.try_get("path").map_err(|_| E::Unavailable)?;
                cursor.clone_from(&path);
                if !r.try_get::<bool, _>("eligible").map_err(|_| E::Unavailable)? {
                    continue;
                }
                pages.push(ReportPage {
                    path,
                    title: r.try_get("title").map_err(|_| E::Unavailable)?,
                    clicks: r.try_get("clicks").map_err(|_| E::Unavailable)?,
                    impressions: r.try_get("impressions").map_err(|_| E::Unavailable)?,
                    position: r.try_get("position").map_err(|_| E::Unavailable)?,
                    measured_days: r.try_get("measured_days").map_err(|_| E::Unavailable)?,
                });
                if pages.len() > 50 {
                    break;
                }
            }
            if pages.len() > 50 || scanned < SCAN_BATCH {
                break;
            }
        }
        let next = if pages.len() > 50 {
            pages.truncate(50);
            pages.last().map(|p| p.path.clone())
        } else {
            None
        };
        tx.commit().await.map_err(|_| E::Unavailable)?;
        Ok(SeoReport {
            days,
            provider,
            sources,
            current,
            previous,
            pages,
            next,
            measured_at,
        })
    }
}
async fn period(
    tx: &mut Transaction<'_, Postgres>,
    id: i64,
    anchor: &str,
    days: i32,
    previous: i32,
) -> Result<PeriodTotals, E> {
    let r=sqlx::query("WITH bounds AS (SELECT ($2::text::date-$3::integer*($4::integer+1)) AS a,($2::text::date-$3::integer*$4::integer) AS b)
        SELECT a::text AS from_day,b::text AS until_day,
          count(d.day) FILTER(WHERE status='available') AS measured_days,
          count(d.total_impressions) AS total_days,sum(d.total_clicks)::bigint AS clicks,sum(d.total_impressions)::bigint AS impressions,
          CASE WHEN count(d.total_impressions) FILTER(WHERE d.total_impressions>0 AND d.total_position IS NULL)=0
            THEN sum(d.total_position*d.total_impressions)/nullif(sum(d.total_impressions),0) END AS position,
          count(d.day) FILTER(WHERE limit_reached) AS limited_days,coalesce(sum(omitted_rows),0)::bigint AS omitted_rows
        FROM bounds LEFT JOIN flow_seo_daily d ON d.source_id=$1 AND d.day>=a AND d.day<b GROUP BY a,b")
        .bind(id).bind(anchor).bind(days).bind(previous).fetch_one(&mut **tx).await.map_err(|_|E::Unavailable)?;
    Ok(PeriodTotals {
        from: r.try_get("from_day").map_err(|_| E::Unavailable)?,
        until: r.try_get("until_day").map_err(|_| E::Unavailable)?,
        measured_days: r.try_get("measured_days").map_err(|_| E::Unavailable)?,
        total_days: r.try_get("total_days").map_err(|_| E::Unavailable)?,
        clicks: r.try_get("clicks").map_err(|_| E::Unavailable)?,
        impressions: r.try_get("impressions").map_err(|_| E::Unavailable)?,
        position: r.try_get("position").map_err(|_| E::Unavailable)?,
        limited_days: r.try_get("limited_days").map_err(|_| E::Unavailable)?,
        omitted_rows: r.try_get("omitted_rows").map_err(|_| E::Unavailable)?,
    })
}
