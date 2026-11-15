WITH anchor AS MATERIALIZED (
    SELECT transaction_timestamp() AS at, $1::integer * interval '24 hours' AS span
), eligible AS MATERIALIZED (
    SELECT id,title,genres,tags FROM reader_return_public_books
    WHERE ($2::text IS NULL OR $2=ANY(genres))
      AND ($3::text IS NULL OR $3=ANY(tags))
      AND (NOT $4::boolean OR cardinality(genres)=0)
      AND (NOT $5::boolean OR cardinality(tags)=0)
), candidates AS MATERIALIZED (
    SELECT DISTINCT c.user_id,c.book_id FROM reader_chapter_completions c
    JOIN eligible b ON b.id=c.book_id CROSS JOIN anchor a
    WHERE c.completed_at>=a.at-a.span*2 AND c.completed_at<a.at
), firsts AS MATERIALIZED (
    SELECT c.user_id,c.book_id,min(c.completed_at) AS completed_at
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
), completed AS (
    SELECT book_id,count(*) FILTER(WHERE completed_at>=a.at-a.span) AS current_pairs,
        count(*) FILTER(WHERE completed_at>=a.at-a.span*2 AND completed_at<a.at-a.span) AS previous_pairs
    FROM firsts CROSS JOIN anchor a GROUP BY book_id
), library AS (
    SELECT l.book_id,count(*) AS links FROM reader_library_items l
    JOIN eligible b ON b.id=l.book_id GROUP BY l.book_id
), follows AS (
    SELECT f.book_id,count(*) AS links FROM reader_book_follows f
    JOIN eligible b ON b.id=f.book_id GROUP BY f.book_id
), per_book AS MATERIALIZED (
    SELECT b.*,COALESCE(c.current_pairs,0) AS current_pairs,
        COALESCE(c.previous_pairs,0) AS previous_pairs,
        COALESCE(l.links,0) AS library_links,COALESCE(f.links,0) AS follow_links
    FROM eligible b LEFT JOIN completed c ON c.book_id=b.id
    LEFT JOIN library l ON l.book_id=b.id LEFT JOIN follows f ON f.book_id=b.id
), totals AS (
    SELECT count(*) AS books,COALESCE(sum(current_pairs),0)::bigint AS current_pairs,
        COALESCE(sum(previous_pairs),0)::bigint AS previous_pairs,
        COALESCE(sum(library_links),0)::bigint AS library_links,
        COALESCE(sum(follow_links),0)::bigint AS follow_links FROM per_book
), membership AS (
    SELECT b.*,x.label FROM per_book b LEFT JOIN LATERAL (
        SELECT DISTINCT item COLLATE "C" AS label
        FROM unnest(CASE WHEN $6::text='genres' THEN b.genres ELSE b.tags END) item
    ) x ON true WHERE $6::text<>'books'
), grouped AS (
    SELECT id AS book_id,title AS label,1::bigint AS books,current_pairs,previous_pairs,library_links,follow_links
    FROM per_book WHERE $6::text='books'
    UNION ALL
    SELECT NULL::bigint,label,count(*),sum(current_pairs)::bigint,sum(previous_pairs)::bigint,
        sum(library_links)::bigint,sum(follow_links)::bigint FROM membership GROUP BY label
), selected AS (
    SELECT * FROM grouped
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
