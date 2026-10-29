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

\echo 'PERF233_SELECTED_SPARSE'
SELECT COALESCE((
    SELECT i.reltuples >= 0 AND b.reltuples > 0
           AND i.reltuples * 5 <= b.reltuples
    FROM pg_catalog.pg_class AS i
    CROSS JOIN pg_catalog.pg_class AS b
    WHERE i.oid = to_regclass('public.books_public_nonempty_taxonomy_idx')
      AND b.oid = to_regclass('public.books')
), FALSE) AS use_sparse;

\echo 'PERF233_TAXONOMY_SPARSE_ONE_PASS'
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
          AND (b.genres <> '{}'::text[] OR b.tags <> '{}'::text[]
               OR b.fandoms <> '{}'::text[])
    ) AS distinct_names
) AS ordered_names
WHERE ordinal <= 100
ORDER BY kind, name;

-- PERF-233 / dense-case isolation: run the exact five-bound-argument
-- count expression used by the Rust PgSearch path, not only the simplified
-- no-filter Oracle above. PostgreSQL may select a different plan for the
-- generic prepared statement with nullable filters.
\echo 'PERF233_COUNT_PARAMETRIC_NO_FILTERS'
PREPARE perf233_count(text,text,text,text,text) AS
SELECT COUNT(*) FROM books
WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
  AND ($1::text = '' OR
       (to_tsvector('simple'::regconfig, translate(lower(normalize(title, NFKC)), 'ё', 'е'))
        || to_tsvector('simple'::regconfig, translate(lower(normalize(author_name, NFKC)), 'ё', 'е')))
           @@ plainto_tsquery('simple'::regconfig, translate(lower(normalize($1::text, NFKC)), 'ё', 'е')))
  AND ($2::text IS NULL OR target_language = $2)
  AND ($3::text IS NULL OR genres @> ARRAY[$3::text])
  AND ($4::text IS NULL OR tags @> ARRAY[$4::text])
  AND ($5::text IS NULL OR fandoms @> ARRAY[$5::text]);
EXPLAIN (ANALYZE, BUFFERS)
EXECUTE perf233_count('', NULL, NULL, NULL, NULL);

\echo 'PERF233_COUNT_PARAMETRIC_DENSE_FACETS'
EXPLAIN (ANALYZE, BUFFERS)
EXECUTE perf233_count('', 'ru', 'f02-load', 'f02-100k-tag', 'f02-100k-fandom');
DEALLOCATE perf233_count;

-- Competing dense-table plan: one filtered heap scan, materialized once,
-- then three independent DISTINCT branches. This is only an A/B candidate;
-- don't replace the verified runtime query before comparing timings/buffers.
\echo 'PERF233_TAXONOMY_DENSE_MATERIALIZED_VISIBLE'
EXPLAIN (ANALYZE, BUFFERS)
WITH visible AS MATERIALIZED (
    SELECT genres,tags,fandoms FROM books
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
)
SELECT 'genre' AS kind, name FROM (
    SELECT DISTINCT unnest(genres) AS name FROM visible
    ORDER BY name LIMIT 100
) g
UNION ALL
SELECT 'tag' AS kind, name FROM (
    SELECT DISTINCT unnest(tags) AS name FROM visible
    ORDER BY name LIMIT 100
) t
UNION ALL
SELECT 'fandom' AS kind, name FROM (
    SELECT DISTINCT unnest(fandoms) AS name FROM visible
    ORDER BY name LIMIT 100
) f;
