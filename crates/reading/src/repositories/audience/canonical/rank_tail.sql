    ORDER BY current_pairs DESC,book_id NULLS FIRST,label COLLATE "C" NULLS FIRST
    LIMIT $7
), result AS (
    SELECT true AS is_total,NULL::bigint AS book_id,NULL::text AS label,t.* FROM totals t
    UNION ALL
    SELECT false,s.* FROM selected s
)
SELECT r.*,to_char(a.at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS measured_at_utc,
    to_char((a.at-a.span) AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS current_from_utc,
    to_char((a.at-a.span*2) AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS previous_from_utc
FROM result r CROSS JOIN anchor a
ORDER BY is_total DESC,current_pairs DESC,book_id NULLS FIRST,label COLLATE "C" NULLS FIRST
