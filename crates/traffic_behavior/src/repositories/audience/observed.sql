WITH anchor AS MATERIALIZED (
    SELECT statement_timestamp() AS at,
           (date_trunc('hour',statement_timestamp() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS until_at
), hours AS (
    SELECT generate_series(a.until_at-($1::integer*2*interval '1 hour'),
        a.until_at-interval '1 hour',interval '1 hour') AS bucket FROM anchor a
), saved AS (
    SELECT f.bucket,
        SUM(html_requests) FILTER (WHERE surface_code=0)::bigint AS views_0,
        SUM(html_requests) FILTER (WHERE surface_code=1)::bigint AS views_1,
        SUM(html_requests) FILTER (WHERE surface_code=2)::bigint AS views_2,
        SUM(html_requests) FILTER (WHERE surface_code=3)::bigint AS views_3,
        SUM(html_requests) FILTER (WHERE acquisition_hint=0)::bigint AS source_0,
        SUM(html_requests) FILTER (WHERE acquisition_hint=1)::bigint AS source_1,
        SUM(html_requests) FILTER (WHERE acquisition_hint=2)::bigint AS source_2,
        SUM(html_requests) FILTER (WHERE acquisition_hint=3)::bigint AS source_3
    FROM ril_flow_observed_hourly f CROSS JOIN anchor a
    WHERE f.bucket>=a.until_at-($1::integer*2*interval '1 hour') AND f.bucket<a.until_at
    GROUP BY f.bucket
), losses AS (
    SELECT COALESCE(SUM(discarded_events),0)::bigint AS discarded
    FROM ril_flow_observed_receipts r CROSS JOIN anchor a
    WHERE r.materialized_at>=a.until_at-($1::integer*interval '1 hour')
        AND r.materialized_at<a.until_at
)
SELECT to_char(a.at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"') AS measured_at_utc,
    EXTRACT(EPOCH FROM a.until_at)::bigint AS until_epoch,
    to_char(a.until_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"') AS until_utc,
    to_char((a.until_at-($1::integer*interval '1 hour')) AT TIME ZONE 'UTC',
        'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS current_from_utc,
    to_char((a.until_at-($1::integer*2*interval '1 hour')) AT TIME ZONE 'UTC',
        'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS previous_from_utc,
    EXTRACT(EPOCH FROM h.bucket)::bigint AS hour_epoch,
    to_char(h.bucket AT TIME ZONE $2::text,'YYYY-MM-DD HH24:MI') AS local_label,
    (EXTRACT(EPOCH FROM ((h.bucket AT TIME ZONE $2::text)
        -(h.bucket AT TIME ZONE 'UTC')))/60)::integer AS offset_minutes,
    (EXTRACT(ISODOW FROM h.bucket AT TIME ZONE $2::text)-1)::integer AS weekday,
    EXTRACT(HOUR FROM h.bucket AT TIME ZONE $2::text)::integer AS local_hour,
    s.views_0,s.views_1,s.views_2,s.views_3,
    s.source_0,s.source_1,s.source_2,s.source_3,l.discarded
FROM anchor a CROSS JOIN hours h CROSS JOIN losses l
LEFT JOIN saved s ON s.bucket=h.bucket ORDER BY h.bucket
