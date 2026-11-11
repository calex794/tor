use super::super::books::sql::BOOKS;

// Expand exactly one taxonomy dimension AFTER complete per-book aggregation.
// DISTINCT prevents legacy duplicate labels from multiplying a book's facts.
const GROUPS: &str = r#",
membership AS MATERIALIZED (
    SELECT b.id AS book_id,x.label
    FROM eligible b
    LEFT JOIN LATERAL (
        SELECT DISTINCT item COLLATE "C" AS label
        FROM unnest(CASE WHEN $6::text='genres' THEN b.genres ELSE b.tags END) item
    ) x ON true
), grouped AS MATERIALIZED (
    SELECT m.label,b.ordinal,count(*) AS matched_books,
           CASE WHEN bool_or(b.issued_incomplete) THEN NULL ELSE sum(b.issued)::bigint END AS issued,
           CASE WHEN bool_or(b.impressions_incomplete) THEN NULL ELSE sum(b.impressions)::bigint END AS impressions,
           CASE WHEN bool_or(b.navigation_incomplete) THEN NULL ELSE sum(b.clicks)::bigint END AS clicks,
           CASE WHEN bool_or(b.opens_incomplete) THEN NULL
                ELSE sum(b.book_opens)::bigint END AS book_opens,
           CASE WHEN bool_or(b.evidence_invalidated) THEN NULL ELSE sum(b.starts)::bigint END AS starts,
           bool_or(b.evidence_invalidated) AS evidence_invalidated,
           count(*) FILTER (WHERE b.evidence_invalidated) AS unknown_evidence,
           count(*) FILTER (WHERE NOT b.evidence_invalidated AND NOT b.observed AND COALESCE(b.starts,0)=0) AS no_observations,
           count(*) FILTER (WHERE NOT b.evidence_invalidated AND b.observed AND COALESCE(b.starts,0)=0) AS observed_without_starts,
           count(*) FILTER (WHERE NOT b.evidence_invalidated AND b.starts>0) AS with_starts
    FROM per_book b JOIN membership m ON m.book_id=b.book_id
    GROUP BY m.label,b.ordinal
), selected_groups AS (
    SELECT label FROM grouped WHERE ordinal=0
      AND ($7::text IS NULL OR label COLLATE "C">$7 COLLATE "C")
    ORDER BY label COLLATE "C" NULLS FIRST LIMIT $8
), result AS (
    SELECT t.ordinal,true AS is_total,NULL::text AS label,
           t.evidence_invalidated,t.issued,t.impressions,t.clicks,t.book_opens,t.starts,t.matched_books,
           t.unknown_evidence,t.no_observations,t.observed_without_starts,t.with_starts
    FROM totals t
    UNION ALL
    SELECT g.ordinal,false,g.label,g.evidence_invalidated,g.issued,g.impressions,g.clicks,g.book_opens,g.starts,
           g.matched_books,g.unknown_evidence,g.no_observations,g.observed_without_starts,g.with_starts
    FROM grouped g JOIN selected_groups s ON s.label IS NOT DISTINCT FROM g.label
), health AS (
    SELECT (SELECT last_success FROM rec_worker_health WHERE singleton) AS last_success,
           EXISTS(SELECT 1 FROM rec_dirty_buckets) AS rec_dirty,
           EXISTS(SELECT 1 FROM organic_discovery_dirty) AS organic_dirty
)
SELECT r.*,
       (SELECT count(*) FROM grouped WHERE ordinal=0) AS matched_groups,
       to_char(a.at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS measured_at,
       to_char(p.from_day,'YYYY-MM-DD') AS from_day,
       to_char(p.until_day,'YYYY-MM-DD') AS until_day,
       p.ordinal=0 AS partial_today,
       to_char(h.last_success AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS last_success,
       COALESCE(h.last_success>a.at-interval '30 seconds' AND h.last_success<=a.at,false) AS fresh,
       h.rec_dirty,h.organic_dirty
FROM result r JOIN periods p ON p.ordinal=r.ordinal CROSS JOIN anchor a CROSS JOIN health h
ORDER BY r.is_total DESC,r.label COLLATE "C" NULLS FIRST,r.ordinal
"#;

pub(super) fn report() -> String {
    format!("{BOOKS}{GROUPS}")
}
