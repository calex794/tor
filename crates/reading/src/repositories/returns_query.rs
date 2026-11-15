//! One repeatable-read statement combines live cohorts and anonymous history.
pub(super) const REPORT: &str = r#"
WITH moment AS MATERIALIZED (
  SELECT now() AS at,(now() AT TIME ZONE 'UTC')::date AS today
), periods AS (
  SELECT 0 AS ordinal,today+1-$1::int AS first_day,today+1 AS last_day FROM moment
  UNION ALL
  SELECT 1,today+1-2*$1::int,today+1-$1::int FROM moment
), observation AS (
  SELECT min(started_at) AS started_at,max(last_heartbeat_at) AS last_observed_at,
         COALESCE(bool_or(ended_at IS NULL AND last_heartbeat_at>=(SELECT at FROM moment)-interval '60 seconds'),false) AS healthy
  FROM reader_return_coverage
), live_horizons AS (
  SELECT s.cohort_day,h.day,h.did_return,
         s.cohort_day+h.day<m.today AND EXISTS(
           SELECT 1 FROM reader_return_coverage e
           WHERE e.started_at<=((s.cohort_day+h.day)::timestamp AT TIME ZONE 'UTC')
             AND COALESCE(e.ended_at,e.last_heartbeat_at)
                 >=((s.cohort_day+h.day+1)::timestamp AT TIME ZONE 'UTC')) AS eligible,
         s.cohort_day+h.day>m.today OR (s.cohort_day+h.day=m.today AND EXISTS(
           SELECT 1 FROM reader_return_coverage e
           WHERE e.started_at<=(m.today::timestamp AT TIME ZONE 'UTC')
             AND e.ended_at IS NULL AND e.last_heartbeat_at>=m.at-interval '60 seconds')) AS pending
  FROM reader_return_subjects s
  CROSS JOIN moment m
  CROSS JOIN LATERAL (VALUES(1,s.returned_d1),(7,s.returned_d7),(30,s.returned_d30)) h(day,did_return)
  WHERE s.cohort_day>=m.today+1-2*$1::int AND s.cohort_day<=m.today
), totals AS (
  SELECT cohort_day,day,count(*) AS enrolled,
         count(*) FILTER(WHERE eligible) AS eligible,
         count(*) FILTER(WHERE eligible AND did_return) AS returned,
         count(*) FILTER(WHERE pending) AS pending,
         count(*) FILTER(WHERE NOT eligible AND NOT pending) AS unknown
  FROM live_horizons GROUP BY cohort_day,day
  UNION ALL
  SELECT d.cohort_day,d.day::int,d.enrolled,d.eligible,d.returned,0::bigint,d.unknown
  FROM reader_return_daily d CROSS JOIN moment m
  WHERE d.cohort_day>=m.today+1-2*$1::int AND d.cohort_day<=m.today
)
SELECT p.ordinal,h.day,
       to_char(m.at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS measured_at,
       to_char(o.started_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS started_at,
       to_char(o.last_observed_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS last_observed_at,
       o.healthy,
       to_char(p.first_day::timestamp,'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS from_utc,
       to_char(p.last_day::timestamp,'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS until_utc,
       EXISTS(SELECT 1 FROM reader_return_coverage e
         WHERE e.started_at<=(p.first_day::timestamp AT TIME ZONE 'UTC')
           AND COALESCE(e.ended_at,e.last_heartbeat_at+interval '60 seconds')
               >=LEAST(m.at,p.last_day::timestamp AT TIME ZONE 'UTC')) AS coverage_complete,
       COALESCE(sum(t.enrolled),0)::bigint AS enrolled,
       COALESCE(sum(t.eligible),0)::bigint AS eligible,
       COALESCE(sum(t.returned),0)::bigint AS returned,
       COALESCE(sum(t.pending),0)::bigint AS pending,
       COALESCE(sum(t.unknown),0)::bigint AS unknown
FROM periods p CROSS JOIN moment m CROSS JOIN observation o
CROSS JOIN (VALUES(1),(7),(30)) h(day)
LEFT JOIN totals t ON t.cohort_day>=p.first_day AND t.cohort_day<p.last_day AND t.day=h.day
GROUP BY p.ordinal,p.first_day,p.last_day,h.day,m.at,o.started_at,o.last_observed_at,o.healthy
ORDER BY p.ordinal,h.day
"#;
