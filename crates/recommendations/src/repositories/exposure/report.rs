use super::*;
use crate::domain::exposure_health::ExposureHealth;
use sqlx::{Postgres, Transaction};

const BASE: &str = r#"WITH bounds AS (
 SELECT CASE WHEN $7::boolean THEN date_trunc('hour',(now()-interval '15 minutes') AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
  ELSE ((now() AT TIME ZONE 'UTC')::date+1)::timestamp AT TIME ZONE 'UTC' END AS until
), eligible AS MATERIALIZED (
 SELECT id,title,genres,tags FROM rec_materializable_books
 WHERE ($3::text IS NULL OR
  ($3='genre' AND (($4='' AND cardinality(genres)=0) OR $4=ANY(genres))) OR
  ($3='tag' AND (($4='' AND cardinality(tags)=0) OR $4=ANY(tags))))
  AND ($8::bigint IS NULL OR id=$8)
), sums AS (
 SELECT h.book_id,CASE WHEN h.bucket>=a.until-make_interval(hours=>$1*24) THEN 0 ELSE 1 END AS period,
  sum(h.opportunities)::bigint AS opportunities,sum(h.rendered)::bigint AS rendered,
  sum(h.views)::bigint AS views,sum(h.clicks)::bigint AS clicks,
  sum(h.expired_unobserved)::bigint AS expired_unobserved,sum(h.replayed)::bigint AS replayed
 FROM flow_observation_hourly h JOIN eligible e ON e.id=h.book_id CROSS JOIN bounds a
 WHERE h.bucket>=a.until-make_interval(hours=>$1*48) AND h.bucket<a.until
  AND ($2::text IS NULL OR h.surface=$2) AND ($5::bigint IS NULL OR h.campaign_id=$5)
  AND ($6::boolean IS NULL OR (h.campaign_id>0)=$6)
 GROUP BY h.book_id,2
), per_book AS MATERIALIZED (
 SELECT e.*,coalesce(c.opportunities,0) AS opportunities,coalesce(c.rendered,0) AS rendered,
  coalesce(c.views,0) AS views,coalesce(c.clicks,0) AS clicks,
  coalesce(c.expired_unobserved,0) AS expired_unobserved,coalesce(c.replayed,0) AS replayed,
  coalesce(p.opportunities,0) AS previous_opportunities,coalesce(p.rendered,0) AS previous_rendered,
  coalesce(p.views,0) AS previous_views,coalesce(p.clicks,0) AS previous_clicks,
  coalesce(p.expired_unobserved,0) AS previous_expired_unobserved,coalesce(p.replayed,0) AS previous_replayed
 FROM eligible e LEFT JOIN sums c ON c.book_id=e.id AND c.period=0
 LEFT JOIN sums p ON p.book_id=e.id AND p.period=1
), classified AS (
 SELECT *,CASE WHEN rendered<20 OR opportunities=0 OR rendered::numeric*100<opportunities::numeric*80 THEN 'unknown'
  WHEN views<5 THEN 'low' ELSE 'observed' END AS class FROM per_book
)
"#;

fn bind<'q>(
    sql: &'q str,
    f: &'q ExposureFilter,
    closed: bool,
    book: Option<i64>,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    sqlx::query(sql)
        .bind(f.days)
        .bind(f.surface.map(|s| s.as_str()))
        .bind(f.group.as_ref().map(|g| g.0.as_str()))
        .bind(f.group.as_ref().map(|g| g.1.as_str()))
        .bind(f.campaign)
        .bind(f.editorial)
        .bind(closed)
        .bind(book)
}
fn sum_columns(prefix: &str) -> String {
    [
        "opportunities",
        "rendered",
        "views",
        "clicks",
        "expired_unobserved",
        "replayed",
    ]
    .into_iter()
    .map(|c| format!("coalesce(sum({prefix}{c}),0)::bigint AS {prefix}{c}"))
    .collect::<Vec<_>>()
    .join(",")
}
impl PgExposure {
    pub async fn report(&self, filter: &ExposureFilter) -> Result<ExposureReport, ExposureError> {
        self.report_with_anchor(filter, false, None).await
    }
    /// Equal completed UTC-hour windows, with a full proof TTL grace before the
    /// latest boundary. Suitable for descriptive change alerts, never causality.
    pub async fn report_closed(
        &self,
        filter: &ExposureFilter,
    ) -> Result<ExposureReport, ExposureError> {
        self.report_with_anchor(filter, true, None).await
    }
    /// Counters, complete genre totals and durable health share one repeatable-read
    /// snapshot. A later checkpoint cannot certify older counter values.
    pub async fn report_closed_with_health(
        &self,
        filter: &ExposureFilter,
    ) -> Result<(ExposureReport, ExposureHealth), ExposureError> {
        validate(filter, None)?;
        let mut tx = self.pool.begin().await?;
        limits(&mut tx).await?;
        let report = report_in(&mut tx, filter, true, None).await?;
        let health = PgExposureHealth::report_in(&mut tx, filter.days, report.until_epoch).await?;
        tx.commit().await?;
        Ok((report, health))
    }
    /// The integration layer obtains this immutable book from the editorial owner,
    /// after administrator authorization. HTTP payloads cannot select this scope.
    pub async fn report_campaign_book(
        &self,
        filter: &ExposureFilter,
        book: i64,
    ) -> Result<ExposureReport, ExposureError> {
        self.report_with_anchor(filter, false, Some(book)).await
    }
    async fn report_with_anchor(
        &self,
        filter: &ExposureFilter,
        closed: bool,
        book: Option<i64>,
    ) -> Result<ExposureReport, ExposureError> {
        validate(filter, book)?;
        let mut tx = self.pool.begin().await?;
        limits(&mut tx).await?;
        let report = report_in(&mut tx, filter, closed, book).await?;
        tx.commit().await?;
        Ok(report)
    }
}
fn validate(filter: &ExposureFilter, book: Option<i64>) -> Result<(), ExposureError> {
    filter.validate()?;
    if filter.campaign.is_some() != book.is_some() || book.is_some_and(|id| id <= 0) {
        return Err(ExposureError::Invalid);
    }
    Ok(())
}
async fn limits(tx: &mut Transaction<'_, Postgres>) -> Result<(), ExposureError> {
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut **tx).await?;
    sqlx::query("SELECT set_config('statement_timeout','750ms',true),set_config('lock_timeout','75ms',true)")
        .execute(&mut **tx).await?;
    Ok(())
}
async fn report_in(
    tx: &mut Transaction<'_, Postgres>,
    filter: &ExposureFilter,
    closed: bool,
    book: Option<i64>,
) -> Result<ExposureReport, ExposureError> {
        let totals=format!("{BASE} SELECT extract(epoch FROM now())::bigint AS checked_at_epoch,(SELECT extract(epoch FROM until)::bigint FROM bounds) AS until_epoch,count(*) AS books,{}, {},count(*) FILTER(WHERE class='low') AS low_books,count(*) FILTER(WHERE class='unknown') AS unknown_books FROM classified",sum_columns(""),sum_columns("previous_"));
        let total = bind(&totals, filter, closed, book)
            .fetch_one(&mut **tx)
            .await?;
        let detail=format!("{BASE} SELECT * FROM classified WHERE id>$9 AND ($10::text IS NULL OR class=$10) ORDER BY id LIMIT 51");
        let rows = bind(&detail, filter, closed, book)
            .bind(filter.after)
            .bind(filter.class.map(|c| c.as_str()))
            .fetch_all(&mut **tx)
            .await?;
        let more = rows.len() > 50;
        let rows = rows
            .into_iter()
            .take(50)
            .map(|row| {
                Ok(ExposureBook {
                    id: row.try_get("id")?,
                    title: row.try_get("title")?,
                    counts: counts(&row, "")?,
                    previous: counts(&row, "previous_")?,
                })
            })
            .collect::<Result<Vec<_>, ExposureError>>()?;
        let next = more.then(|| rows.last().map(|r| r.id)).flatten();
        // A book contributes once within each group, never once per duplicate label.
        // The list is bounded; any exact group can also be selected through the form.
        let grouped = format!(
            r#"{BASE} SELECT g.kind,g.label,count(*) AS books,{}
          FROM classified b CROSS JOIN LATERAL (
            SELECT DISTINCT 'genre' AS kind,v AS label FROM unnest(CASE WHEN cardinality(b.genres)=0 THEN ARRAY['']::text[] ELSE b.genres END) v
            UNION SELECT DISTINCT 'tag',v FROM unnest(CASE WHEN cardinality(b.tags)=0 THEN ARRAY['']::text[] ELSE b.tags END) v
          ) g GROUP BY g.kind,g.label ORDER BY g.kind COLLATE "C",g.label COLLATE "C" LIMIT 100"#,
            sum_columns("")
        );
        let groups = bind(&grouped, filter, closed, book)
            .fetch_all(&mut **tx)
            .await?
            .into_iter()
            .map(|row| {
                Ok(ExposureGroup {
                    kind: row.try_get("kind")?,
                    label: row.try_get("label")?,
                    books: row.try_get("books")?,
                    counts: counts(&row, "")?,
                })
            })
            .collect::<Result<Vec<_>, ExposureError>>()?;
        let genre_sql = format!(r#"{BASE}, genre_counts AS (
            SELECT g.label,sum(opportunities) AS opportunities,sum(rendered) AS rendered,sum(views) AS views
            FROM classified b CROSS JOIN LATERAL (
                SELECT DISTINCT v AS label FROM unnest(b.genres) v WHERE btrim(v)<>''
            ) g GROUP BY g.label
        ), genre_classes AS (
            SELECT label,CASE WHEN rendered<20 OR opportunities=0 OR rendered*100<opportunities*80 THEN 'unknown'
              WHEN views<5 THEN 'low' ELSE 'observed' END AS class FROM genre_counts
        ) SELECT count(*) AS genres,count(*) FILTER(WHERE class='low') AS low,
          count(*) FILTER(WHERE class='unknown') AS unknown,
          ARRAY(SELECT label FROM genre_classes WHERE class='low' ORDER BY label COLLATE "C" LIMIT 20) AS low_labels
          FROM genre_classes"#);
        let genre = bind(&genre_sql, filter, closed, book).fetch_one(&mut **tx).await?;
        Ok(ExposureReport {
            checked_at_epoch: total.try_get("checked_at_epoch")?,
            until_epoch: total.try_get("until_epoch")?,
            books: total.try_get("books")?,
            current: counts(&total, "")?,
            previous: counts(&total, "previous_")?,
            low_observed_books: total.try_get("low_books")?,
            unknown_books: total.try_get("unknown_books")?,
            genres: GenreExposureSummary {
                genres: genre.try_get("genres")?,
                low_observed: genre.try_get("low")?,
                unknown: genre.try_get("unknown")?,
                low_labels: genre.try_get("low_labels")?,
            },
            rows,
            next,
            groups,
        })
}
