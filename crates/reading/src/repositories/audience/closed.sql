WITH anchor AS MATERIALIZED (
    SELECT date_trunc('hour',transaction_timestamp() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC' AS at,
        $1::integer*interval '24 hours' AS span
), periods AS (
    SELECT 0 AS ordinal,at-span AS from_at,at AS until_at FROM anchor
    UNION ALL
    SELECT 1,at-span*2,at-span FROM anchor
), starts AS (
    SELECT c.started_at AS occurred_at,0 AS kind
    FROM reader_chapter_starts c JOIN outbox_events e ON e.id=c.source_event_id
    CROSS JOIN anchor a
    WHERE c.started_at>=a.at-a.span*2 AND c.started_at<a.at
      AND e.event_type='ChapterStartedV1' AND e.schema_version=1
      AND e.aggregate_kind='chapter' AND e.aggregate_id=c.chapter_id
      AND e.payload->>'user_id'=c.user_id::text
      AND e.payload->>'book_id'=c.book_id::text
      AND e.payload->>'chapter_id'=c.chapter_id::text
      AND e.payload->>'version_id'=c.first_version_id::text
      AND ((c.purchase_id IS NULL AND e.payload->>'authority'='C-verified-free-start'
            AND NOT(e.payload ? 'purchase_id'))
        OR (c.purchase_id IS NOT NULL AND e.payload->>'authority'='C-verified-purchased-start'
            AND e.payload->>'purchase_id'=c.purchase_id::text))
), candidates AS MATERIALIZED (
    SELECT DISTINCT c.user_id,c.book_id FROM reader_chapter_completions c
    JOIN reader_return_public_books b ON b.id=c.book_id CROSS JOIN anchor a
    WHERE c.completed_at>=a.at-a.span*2 AND c.completed_at<a.at
), firsts AS MATERIALIZED (
    SELECT c.user_id,c.book_id,min(c.completed_at) AS occurred_at
    FROM reader_chapter_completions c
    JOIN candidates p ON p.user_id=c.user_id AND p.book_id=c.book_id
    JOIN outbox_events e ON e.id=c.source_event_id CROSS JOIN anchor a
    WHERE c.completed_at<a.at
      AND e.event_type='ChapterCompletedV1' AND e.schema_version=1
      AND e.aggregate_kind='chapter' AND e.aggregate_id=c.chapter_id
      AND e.payload->>'user_id'=c.user_id::text
      AND e.payload->>'book_id'=c.book_id::text
      AND e.payload->>'chapter_id'=c.chapter_id::text
      AND e.payload->>'version_id'=c.first_version_id::text
      AND ((c.purchase_id IS NULL AND e.payload->>'authority'='C-verified-free-position'
            AND NOT(e.payload ? 'purchase_id'))
        OR (c.purchase_id IS NOT NULL AND e.payload->>'authority'='C-verified-purchased-position'
            AND e.payload->>'purchase_id'=c.purchase_id::text))
    GROUP BY c.user_id,c.book_id
), evidence AS (
    SELECT occurred_at,kind FROM starts
    UNION ALL
    SELECT occurred_at,1 FROM firsts
)
SELECT p.ordinal,extract(epoch FROM a.at)::bigint AS until_epoch,
    to_char(transaction_timestamp() AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS measured_at_utc,
    count(*) FILTER(WHERE e.kind=0) AS starts,
    count(*) FILTER(WHERE e.kind=1) AS completed_book_pairs
FROM periods p CROSS JOIN anchor a LEFT JOIN evidence e
    ON e.occurred_at>=p.from_at AND e.occurred_at<p.until_at
GROUP BY p.ordinal,a.at ORDER BY p.ordinal
