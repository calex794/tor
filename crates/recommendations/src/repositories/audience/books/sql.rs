// Every retained bucket contributes before the book cursor is applied. Current
// public eligibility is deliberately different from the unnamed source report.
pub(in crate::repositories::audience) const BOOKS: &str = r#"
WITH anchor AS (
    SELECT transaction_timestamp() AS at,
           (transaction_timestamp() AT TIME ZONE 'UTC')::date+1 AS until_day
), periods AS (
    SELECT 0 AS ordinal, until_day-$1::int AS from_day, until_day FROM anchor
    UNION ALL
    SELECT 1, until_day-$1::int*2, until_day-$1::int FROM anchor
), available AS MATERIALIZED (
    SELECT id,title,genres,tags FROM rec_public_books
), eligible AS MATERIALIZED (
    SELECT * FROM available
    WHERE ($2::text IS NULL OR $2=ANY(genres))
      AND ($3::text IS NULL OR $3=ANY(tags))
      AND (NOT $4::boolean OR cardinality(genres)=0)
      AND (NOT $5::boolean OR cardinality(tags)=0)
), facts AS (
    SELECT q.book_id,
           CASE WHEN q.bucket_date>=a.until_day-$1::int THEN 0 ELSE 1 END AS ordinal,
           NULL::bigint AS issued,q.impressions,
           q.paired_clicks+q.unpaired_clicks AS clicks,q.book_opens,q.starts,
           q.book_opens IS NULL AS opens_incomplete
    FROM rec_quality_daily q JOIN eligible b ON b.id=q.book_id CROSS JOIN anchor a
    WHERE q.bucket_date>=a.until_day-$1::int*2 AND q.bucket_date<a.until_day
    UNION ALL
    SELECT q.book_id,
           CASE WHEN q.bucket_date>=a.until_day-$1::int THEN 0 ELSE 1 END,
           CASE WHEN q.source<>'unattributed' THEN q.issued END,NULL::bigint,
           CASE WHEN q.source<>'unattributed' THEN q.clicks END,
           CASE WHEN q.source<>'unattributed' THEN q.book_opens END,q.starts,false
    FROM organic_discovery_quality_daily q
    JOIN eligible b ON b.id=q.book_id CROSS JOIN anchor a
    WHERE q.bucket_date>=a.until_day-$1::int*2 AND q.bucket_date<a.until_day
      AND q.source IN ('catalog','search','unattributed')
), sums AS (
    SELECT book_id,ordinal,sum(issued)::bigint AS issued,
           sum(impressions)::bigint AS impressions,sum(clicks)::bigint AS clicks,
           CASE WHEN bool_or(opens_incomplete) THEN NULL
                ELSE sum(book_opens)::bigint END AS book_opens,
           sum(starts)::bigint AS starts,bool_or(opens_incomplete) AS opens_incomplete
    FROM facts GROUP BY book_id,ordinal
), per_book AS MATERIALIZED (
    SELECT b.id AS book_id,b.title,b.genres,b.tags,p.ordinal,
           s.issued,s.impressions,s.clicks,s.book_opens,s.starts,
           COALESCE(s.opens_incomplete,false) AS opens_incomplete,
           GREATEST(COALESCE(s.issued,0),COALESCE(s.impressions,0),
                    COALESCE(s.clicks,0),COALESCE(s.book_opens,0))>0 AS observed
    FROM eligible b CROSS JOIN periods p
    LEFT JOIN sums s ON s.book_id=b.id AND s.ordinal=p.ordinal
), totals AS (
    SELECT p.ordinal,sum(b.issued)::bigint AS issued,
           sum(b.impressions)::bigint AS impressions,sum(b.clicks)::bigint AS clicks,
           CASE WHEN bool_or(b.opens_incomplete) THEN NULL
                ELSE sum(b.book_opens)::bigint END AS book_opens,
           sum(b.starts)::bigint AS starts,count(b.book_id) AS matched_books,
           count(*) FILTER (WHERE b.book_id IS NOT NULL AND NOT b.observed
                            AND COALESCE(b.starts,0)=0) AS no_observations,
           count(*) FILTER (WHERE b.observed AND COALESCE(b.starts,0)=0) AS observed_without_starts,
           count(*) FILTER (WHERE b.starts>0) AS with_starts
    FROM periods p LEFT JOIN per_book b ON b.ordinal=p.ordinal GROUP BY p.ordinal
)
"#;

const DETAIL: &str = r#",
selected AS (
    SELECT id FROM eligible WHERE $6::bigint IS NULL OR id>$6
    ORDER BY id LIMIT $7
), result AS (
    SELECT t.ordinal,true AS is_total,NULL::bigint AS book_id,NULL::text AS title,
           NULL::text[] AS genres,NULL::text[] AS tags,
           t.issued,t.impressions,t.clicks,t.book_opens,t.starts,t.matched_books,
           t.no_observations,t.observed_without_starts,t.with_starts
    FROM totals t
    UNION ALL
    SELECT b.ordinal,false,b.book_id,b.title,b.genres,b.tags,
           b.issued,b.impressions,b.clicks,b.book_opens,b.starts,
           NULL::bigint,NULL::bigint,NULL::bigint,NULL::bigint
    FROM per_book b JOIN selected s ON s.id=b.book_id
), health AS (
    SELECT (SELECT last_success FROM rec_worker_health WHERE singleton) AS last_success,
           EXISTS(SELECT 1 FROM rec_dirty_buckets) AS rec_dirty,
           EXISTS(SELECT 1 FROM organic_discovery_dirty) AS organic_dirty
)
SELECT r.*,
       to_char(a.at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS measured_at,
       to_char(p.from_day,'YYYY-MM-DD') AS from_day,
       to_char(p.until_day,'YYYY-MM-DD') AS until_day,
       p.ordinal=0 AS partial_today,
       to_char(h.last_success AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS last_success,
       COALESCE(h.last_success>a.at-interval '30 seconds' AND h.last_success<=a.at,false) AS fresh,
       h.rec_dirty,h.organic_dirty,
       CASE WHEN r.is_total AND r.ordinal=0 THEN ARRAY(
           SELECT DISTINCT label COLLATE "C" FROM available b CROSS JOIN unnest(b.genres) label
           ORDER BY 1 LIMIT $8
       ) ELSE ARRAY[]::text[] END AS genre_options,
       CASE WHEN r.is_total AND r.ordinal=0 THEN ARRAY(
           SELECT DISTINCT label COLLATE "C" FROM available b CROSS JOIN unnest(b.tags) label
           ORDER BY 1 LIMIT $8
       ) ELSE ARRAY[]::text[] END AS tag_options
FROM result r JOIN periods p ON p.ordinal=r.ordinal CROSS JOIN anchor a CROSS JOIN health h
ORDER BY r.is_total DESC,r.book_id,r.ordinal
"#;

pub(super) fn report() -> String {
    format!("{BOOKS}{DETAIL}")
}
