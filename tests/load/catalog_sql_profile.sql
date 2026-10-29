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

\echo 'PERF233_TAXONOMY_NEW_ONE_SCAN'
EXPLAIN (ANALYZE, BUFFERS)
SELECT kind, name FROM (
    SELECT kind, name,
           ROW_NUMBER() OVER (PARTITION BY kind ORDER BY name) AS ordinal
    FROM (
        SELECT DISTINCT facet.kind, item.name
        FROM books AS b
        CROSS JOIN LATERAL (
            VALUES ('genre'::text, b.genres),
                   ('tag'::text, b.tags),
                   ('fandom'::text, b.fandoms)
        ) AS facet(kind, entries)
        CROSS JOIN LATERAL unnest(facet.entries) AS item(name)
        WHERE b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
          AND (cardinality(b.genres) > 0 OR cardinality(b.tags) > 0
               OR cardinality(b.fandoms) > 0)
    ) AS distinct_names
) AS ordered_names
WHERE ordinal <= 100
ORDER BY kind, name;
