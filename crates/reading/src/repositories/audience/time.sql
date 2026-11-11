WITH anchor AS MATERIALIZED (
    SELECT date_trunc('hour',transaction_timestamp() AT TIME ZONE 'UTC')
        AT TIME ZONE 'UTC' AS at, $1::integer * interval '24 hours' AS span,
        transaction_timestamp() AS measured
), periods AS MATERIALIZED (
    SELECT 0 AS ordinal,at-span AS from_at,at AS until_at,at,measured FROM anchor
    UNION ALL
    SELECT 1,at-span*2,at-span,at,measured FROM anchor
), evidence AS MATERIALIZED (
    SELECT c.started_at AS occurred_at,0 AS kind,
        COALESCE(e.event_type='ChapterStartedV1' AND e.schema_version=1
          AND e.aggregate_kind='chapter' AND e.aggregate_id=c.chapter_id
          AND e.payload->>'user_id'=c.user_id::text
          AND e.payload->>'book_id'=c.book_id::text
          AND e.payload->>'chapter_id'=c.chapter_id::text
          AND e.payload->>'version_id'=c.first_version_id::text
          AND ((c.purchase_id IS NULL AND e.payload->>'authority'='C-verified-free-start'
                AND NOT(e.payload ? 'purchase_id'))
            OR (c.purchase_id IS NOT NULL AND e.payload->>'authority'='C-verified-purchased-start'
                AND e.payload->>'purchase_id'=c.purchase_id::text)),false) AS verified
    FROM reader_chapter_starts c LEFT JOIN outbox_events e ON e.id=c.source_event_id
    CROSS JOIN anchor a WHERE c.started_at>=a.at-a.span*2 AND c.started_at<a.at
    UNION ALL
    SELECT c.completed_at,1,
        COALESCE(e.event_type='ChapterCompletedV1' AND e.schema_version=1
          AND e.aggregate_kind='chapter' AND e.aggregate_id=c.chapter_id
          AND e.payload->>'user_id'=c.user_id::text
          AND e.payload->>'book_id'=c.book_id::text
          AND e.payload->>'chapter_id'=c.chapter_id::text
          AND e.payload->>'version_id'=c.first_version_id::text
          AND ((c.purchase_id IS NULL AND e.payload->>'authority'='C-verified-free-position'
                AND NOT(e.payload ? 'purchase_id'))
            OR (c.purchase_id IS NOT NULL AND e.payload->>'authority'='C-verified-purchased-position'
                AND e.payload->>'purchase_id'=c.purchase_id::text)),false)
    FROM reader_chapter_completions c LEFT JOIN outbox_events e ON e.id=c.source_event_id
    CROSS JOIN anchor a WHERE c.completed_at>=a.at-a.span*2 AND c.completed_at<a.at
), grouped AS MATERIALIZED (
    -- Local categories are derived from the actual fact timestamp. The allowed
    -- zones have whole-hour offsets/transitions throughout these recent windows.
    SELECT date_trunc('hour',occurred_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC' AS bucket,
        extract(hour FROM occurred_at AT TIME ZONE $2)::integer AS local_hour,
        extract(isodow FROM occurred_at AT TIME ZONE $2)::integer-1 AS weekday,
        count(*) FILTER(WHERE kind=0 AND verified) AS starts,
        count(*) FILTER(WHERE kind=1 AND verified) AS chapter_completions,
        count(*) FILTER(WHERE NOT verified) AS unmatched
    FROM evidence GROUP BY 1,2,3
), slots AS (
    SELECT p.*,s.bucket,
        extract(hour FROM s.bucket AT TIME ZONE $2)::integer AS local_hour,
        extract(isodow FROM s.bucket AT TIME ZONE $2)::integer-1 AS weekday
    FROM periods p CROSS JOIN LATERAL
        generate_series(p.from_at,p.until_at-interval '1 hour',interval '1 hour') AS s(bucket)
)
SELECT s.ordinal,extract(epoch FROM s.at)::bigint AS until_epoch,
    extract(epoch FROM s.from_at)::bigint AS from_epoch,
    extract(epoch FROM s.until_at)::bigint AS period_until_epoch,
    to_char(s.measured AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS measured_at_utc,
    to_char(s.from_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS from_utc,
    to_char(s.until_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS until_utc,
    extract(epoch FROM s.bucket)::bigint AS epoch,
    to_char(s.bucket AT TIME ZONE $2,'YYYY-MM-DD HH24:MI') AS local_label,
    (extract(epoch FROM ((s.bucket AT TIME ZONE $2)
        -(s.bucket AT TIME ZONE 'UTC')))/60)::integer AS offset_minutes,
    s.local_hour,s.weekday,
    COALESCE(g.starts,0)::bigint AS starts,
    COALESCE(g.chapter_completions,0)::bigint AS chapter_completions,
    COALESCE(g.unmatched,0)::bigint AS unmatched
FROM slots s LEFT JOIN grouped g ON g.bucket=s.bucket
    AND g.local_hour=s.local_hour AND g.weekday=s.weekday
ORDER BY s.ordinal,s.bucket
