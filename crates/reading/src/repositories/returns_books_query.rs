//! One snapshot combines live bounded book observations with anonymous totals.
// Only these static prefixes vary; all cohort, maturity and coverage semantics
// below are shared by book and group reports. No caller input enters SQL text.
pub(super) fn book_report() -> String {
    [BOOK_SELECTION, REPORT].concat()
}

pub(super) fn group_report() -> String {
    [GROUP_SELECTION, REPORT].concat()
}

const BOOK_SELECTION: &str = r#"
WITH selected_books AS MATERIALIZED (
  SELECT id,title FROM reader_return_public_books WHERE id=$2
), selected AS (
  SELECT id AS book_id,title AS book_title,1::bigint AS matched_books FROM selected_books
)
"#;

const GROUP_SELECTION: &str = r#"
WITH selected_books AS MATERIALIZED (
  SELECT DISTINCT id FROM reader_return_public_books
  WHERE CASE WHEN $3::text IS NULL
    THEN cardinality(CASE WHEN $2::text='genres' THEN genres ELSE tags END)=0
    ELSE $3::text=ANY(CASE WHEN $2::text='genres' THEN genres ELSE tags END)
  END
), selected AS (
  SELECT NULL::bigint AS book_id,NULL::text AS book_title,count(*)::bigint AS matched_books
  FROM selected_books
)
"#;

const REPORT: &str = r#"
, moment AS MATERIALIZED (
  SELECT now() AS at,(now() AT TIME ZONE 'UTC')::date AS today
), periods AS (
  SELECT 0 AS ordinal,today+1-$1::int AS first_day,today+1 AS last_day FROM moment
  UNION ALL
  SELECT 1,today+1-2*$1::int,today+1-$1::int FROM moment
), observation AS (
  SELECT min(started_at) AS started_at,max(last_heartbeat_at) AS last_observed_at,
         COALESCE(bool_or(ended_at IS NULL AND last_heartbeat_at>=(SELECT at FROM moment)-interval '60 seconds'),false) AS healthy
  FROM reader_book_return_coverage
), live_horizons AS (
  SELECT s.cohort_day,h.day,h.did_return,
         s.cohort_day+h.day<m.today AND EXISTS(
           SELECT 1 FROM reader_book_return_coverage e
           WHERE e.started_at<=((s.cohort_day+h.day)::timestamp AT TIME ZONE 'UTC')
             AND COALESCE(e.ended_at,e.last_heartbeat_at)
                 >=((s.cohort_day+h.day+1)::timestamp AT TIME ZONE 'UTC')) AS eligible,
         s.cohort_day+h.day>m.today OR (s.cohort_day+h.day=m.today AND EXISTS(
           SELECT 1 FROM reader_book_return_coverage e
           WHERE e.started_at<=(m.today::timestamp AT TIME ZONE 'UTC')
             AND e.ended_at IS NULL AND e.last_heartbeat_at>=m.at-interval '60 seconds')) AS pending
  FROM reader_book_return_subjects s JOIN selected_books b ON b.id=s.book_id
  CROSS JOIN moment m
  CROSS JOIN LATERAL (VALUES(7,s.returned_d7),(30,s.returned_d30)) h(day,did_return)
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
  FROM reader_book_return_daily d JOIN selected_books b ON b.id=d.book_id
  CROSS JOIN moment m
  WHERE d.cohort_day>=m.today+1-2*$1::int AND d.cohort_day<=m.today
)
SELECT b.book_id,b.book_title,b.matched_books,p.ordinal,h.day,
       to_char(m.at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS measured_at,
       to_char(o.started_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS started_at,
       to_char(o.last_observed_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS last_observed_at,
       o.healthy,
       to_char(p.first_day::timestamp,'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS from_utc,
       to_char(p.last_day::timestamp,'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS until_utc,
       EXISTS(SELECT 1 FROM reader_book_return_coverage e
         WHERE e.started_at<=(p.first_day::timestamp AT TIME ZONE 'UTC')
           AND COALESCE(e.ended_at,e.last_heartbeat_at+interval '60 seconds')
               >=LEAST(m.at,p.last_day::timestamp AT TIME ZONE 'UTC')) AS coverage_complete,
       COALESCE(sum(t.enrolled),0)::bigint AS enrolled,
       COALESCE(sum(t.eligible),0)::bigint AS eligible,
       COALESCE(sum(t.returned),0)::bigint AS returned,
       COALESCE(sum(t.pending),0)::bigint AS pending,
       COALESCE(sum(t.unknown),0)::bigint AS unknown
FROM selected b CROSS JOIN periods p CROSS JOIN moment m CROSS JOIN observation o
CROSS JOIN (VALUES(7),(30)) h(day)
LEFT JOIN totals t ON t.cohort_day>=p.first_day AND t.cohort_day<p.last_day AND t.day=h.day
GROUP BY b.book_id,b.book_title,b.matched_books,p.ordinal,p.first_day,p.last_day,h.day,m.at,o.started_at,o.last_observed_at,o.healthy
ORDER BY p.ordinal,h.day
"#;
