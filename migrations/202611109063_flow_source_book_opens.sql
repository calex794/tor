-- F-owned anonymous daily totals: an unmeasured historical opening is not zero.
ALTER TABLE rec_quality_daily ADD COLUMN book_opens bigint
    CHECK (book_opens >= 0);

-- Only complete, unexpired raw cohorts can recover this previously absent metric.
-- Update this one field in place. Never enqueue old cohorts for DELETE/rebuild:
-- a delayed worker could otherwise replace retained history with expired raw data.
WITH measured AS (
    SELECT cohort_day, book_id, surface, source, reason, position,
           COALESCE(generation_id::text, 'fallback') AS generation,
           algorithm_version,
           count(*) FILTER (WHERE book_opened_at IS NOT NULL) AS book_opens
    FROM rec_assignments
    WHERE cohort_day >= (now() AT TIME ZONE 'UTC')::date - 29
      AND cohort_day <= (now() AT TIME ZONE 'UTC')::date
    GROUP BY cohort_day, book_id, surface, source, reason, position,
             generation_id, algorithm_version
)
UPDATE rec_quality_daily q
SET book_opens = m.book_opens
FROM measured m
WHERE q.bucket_date = m.cohort_day AND q.book_id = m.book_id
  AND q.surface = m.surface AND q.source = m.source AND q.reason = m.reason
  AND q.position = m.position AND q.generation = m.generation
  AND q.algorithm_version = m.algorithm_version AND q.book_opens IS NULL;
