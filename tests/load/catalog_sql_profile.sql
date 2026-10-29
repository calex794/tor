-- PERF-233: read-only A/B plans on the same disposable PostgreSQL.
-- This script is run after the test fixture is seeded and ANALYZE books.
-- It does not modify production schemas or store user input.
\echo 'PERF233_COUNT_EXACT'
EXPLAIN (ANALYZE, BUFFERS)
SELECT COUNT(*)
FROM books
WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted;

\echo 'PERF233_TAXONOMY_OLD_THREE_SCANS'
EXPLAIN (ANALYZE, BUFFERS)
SELECT 'genre' AS kind, name FROM (
    SELECT DISTINCT unnest(genres) AS name FROM books
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
    ORDER BY name LIMIT 100
) g
UNION ALL
SELECT 'tag' AS kind, name FROM (
    SELECT DISTINCT unnest(tags) AS name FROM books
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
    ORDER BY name LIMIT 100
) t
UNION ALL
SELECT 'fandom' AS kind, name FROM (
    SELECT DISTINCT unnest(fandoms) AS name FROM books
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
    ORDER BY name LIMIT 100
) f;

\echo 'PERF233_TAXONOMY_SPARSE_INDEXED'
EXPLAIN (ANALYZE, BUFFERS)
SELECT 'genre' AS kind, name FROM (
    SELECT DISTINCT unnest(genres) AS name FROM books
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
      AND cardinality(genres) > 0
    ORDER BY name LIMIT 100
) g
UNION ALL
SELECT 'tag' AS kind, name FROM (
    SELECT DISTINCT unnest(tags) AS name FROM books
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
      AND cardinality(tags) > 0
    ORDER BY name LIMIT 100
) t
UNION ALL
SELECT 'fandom' AS kind, name FROM (
    SELECT DISTINCT unnest(fandoms) AS name FROM books
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
      AND cardinality(fandoms) > 0
    ORDER BY name LIMIT 100
) f;
