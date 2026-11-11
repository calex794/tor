use super::*;
use crate::domain::html_health::{HtmlHealthPeriod,HtmlHealthReport};
impl PgHtmlHealth {
    pub async fn report(&self,days:i32,until_epoch:i64)->Result<HtmlHealthReport,HtmlHealthError>{
        let mut tx=self.pool.begin().await?;
        limits(&mut tx,true).await?;
        let report=Self::read_in(&mut tx,days,until_epoch).await?;
        tx.commit().await?;
        Ok(report)
    }
    pub(crate) async fn read_in(tx:&mut Transaction<'_,Postgres>,days:i32,until_epoch:i64)->Result<HtmlHealthReport,HtmlHealthError>{
        if !matches!(days,1|7|30|90) || until_epoch<=0 || until_epoch%3600!=0 {return Err(HtmlHealthError::Invalid);}
        let valid:bool=sqlx::query_scalar("SELECT $1<=extract(epoch FROM date_trunc('hour',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')::bigint AND $1>=extract(epoch FROM now()-interval '3 hours')::bigint")
            .bind(until_epoch).fetch_one(&mut **tx).await?;
        if !valid {return Err(HtmlHealthError::Invalid);}
        let rows=sqlx::query(r#"WITH hours AS (
          SELECT b,CASE WHEN b>=($2::bigint-$1::bigint*86400)*1000 THEN 0 ELSE 1 END AS period
          FROM generate_series(($2::bigint-$1::bigint*172800)*1000,$2::bigint*1000-3600000,3600000) b
        ), measured AS (
          SELECT h.period,q.covered,coalesce(d.loss,false) AS loss,coalesce(d.error,false) AS error
          FROM hours h LEFT JOIN LATERAL (
            SELECT coalesce(range_agg(int8range(e.started_ms,e.confirmed_ms,'[)')) @> int8range(h.b,h.b+3600000,'[)'),false)
              AND coalesce(bool_and(e.confirmed_ms>=CASE WHEN e.retired THEN least(e.lease_until_ms,h.b+3600000) ELSE h.b+3600000 END),false) AS covered
            FROM ril_flow_health_epochs e
            WHERE int8range(e.started_ms,CASE WHEN e.retired THEN e.lease_until_ms ELSE NULL END,'[)') && int8range(h.b,h.b+3600000,'[)')
          ) q ON true LEFT JOIN LATERAL (
            SELECT bool_or(had_loss) AS loss,bool_or(had_error) AS error
            FROM ril_flow_health_hourly WHERE bucket_ms=h.b
          ) d ON true
        ) SELECT period,count(*) FILTER(WHERE covered)::integer AS confirmed_hours,
          count(*) FILTER(WHERE loss)::integer AS loss_hours,count(*) FILTER(WHERE error)::integer AS error_hours
          FROM measured GROUP BY period ORDER BY period"#)
            .bind(days).bind(until_epoch).fetch_all(&mut **tx).await?;
        if rows.len()!=2 {return Err(HtmlHealthError::Unavailable);}
        let period=|row:&sqlx::postgres::PgRow|->Result<HtmlHealthPeriod,HtmlHealthError>{Ok(HtmlHealthPeriod{
            confirmed_hours:row.try_get("confirmed_hours")?,loss_hours:row.try_get("loss_hours")?,error_hours:row.try_get("error_hours")?
        })};
        Ok(HtmlHealthReport{until_epoch,current:period(&rows[0])?,previous:period(&rows[1])?})
    }
}
