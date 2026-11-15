WITH anchor AS MATERIALIZED (
    SELECT to_timestamp($1::bigint::double precision) AS at,
        $2::integer * interval '24 hours' AS span
), requested AS MATERIALIZED (
    SELECT unnest($3::bigint[]) AS book_id
), eligible AS MATERIALIZED (
    SELECT b.id FROM reader_return_public_books b JOIN requested r ON r.book_id=b.id
), periods AS (
    SELECT 0 AS ordinal,at-span AS from_at,at AS until_at FROM anchor
    UNION ALL
    SELECT 1,at-span*2,at-span FROM anchor
), evidence AS MATERIALIZED (
    SELECT c.book_id,c.started_at AS occurred_at,0 AS kind,
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
    FROM reader_chapter_starts c JOIN eligible b ON b.id=c.book_id
    LEFT JOIN outbox_events e ON e.id=c.source_event_id CROSS JOIN anchor a
    WHERE c.started_at>=a.at-a.span*2 AND c.started_at<a.at
    UNION ALL
    SELECT c.book_id,c.completed_at,1,
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
    FROM reader_chapter_completions c JOIN eligible b ON b.id=c.book_id
    LEFT JOIN outbox_events e ON e.id=c.source_event_id CROSS JOIN anchor a
    WHERE c.completed_at>=a.at-a.span*2 AND c.completed_at<a.at
)
SELECT r.book_id,p.ordinal,b.id IS NOT NULL AS eligible,
    count(*) FILTER(WHERE e.kind=0 AND e.verified) AS starts,
    count(*) FILTER(WHERE e.kind=1 AND e.verified) AS chapter_completions,
    count(*) FILTER(WHERE NOT e.verified) AS unmatched
FROM requested r CROSS JOIN periods p LEFT JOIN eligible b ON b.id=r.book_id
LEFT JOIN evidence e ON e.book_id=r.book_id
    AND e.occurred_at>=p.from_at AND e.occurred_at<p.until_at
GROUP BY r.book_id,p.ordinal,b.id ORDER BY r.book_id,p.ordinal
