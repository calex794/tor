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


-- PERF-04 / SEARCH-DENSE: actual F-03 oldest keyset query, including the
-- NFKC + lower + yo-to-e normalization. This is NOT the earlier raw-title
-- index expression. Read-only EXPLAIN on isolated tor_ci, never production.
-- These 10k fixture queries mirror the public REST endpoint with
-- include_total=false, so no misleading COUNT is included in their timing.
\echo 'PERF04_FTS_VISIBLE_INDEX_DEFINITIONS'
SELECT indexname, indexdef
  FROM pg_indexes
 WHERE schemaname='public' AND tablename='books'
   AND (indexdef ILIKE '%to_tsvector%' OR indexname LIKE '%public%')
 ORDER BY indexname;

\echo 'PERF04_FTS_EXACT_OLDEST_SQL_PREPARE'
PREPARE perf04_public_fts(text, bigint, bigint, text, text, text, text) AS
SELECT id, title, target_language, author_name, genres, tags, fandoms
FROM books
WHERE visibility='public' AND visibility_status='active'
  AND access_level='basic' AND lifecycle_status<>'pending_deletion'
  AND NOT admin_hold AND NOT is_deleted
  AND ($1::text = '' OR
       (to_tsvector('simple'::regconfig, translate(lower(normalize(title, NFKC)), 'ё', 'е'))
        || to_tsvector('simple'::regconfig, translate(lower(normalize(author_name, NFKC)), 'ё', 'е')))
           @@ plainto_tsquery('simple'::regconfig,
                               translate(lower(normalize($1::text, NFKC)), 'ё', 'е')))
  AND id > $2
  AND ($4::text IS NULL OR target_language = $4)
  AND ($5::text IS NULL OR genres @> ARRAY[$5::text])
  AND ($6::text IS NULL OR tags @> ARRAY[$6::text])
  AND ($7::text IS NULL OR fandoms @> ARRAY[$7::text])
ORDER BY id ASC LIMIT $3;

-- The same 21-row request as Rust (20 cards + cursor probe).
-- The seed intentionally contains roughly 10k 'Load' matches, one '9999'.
-- EXPLAIN (ANALYZE, BUFFERS) executes the SELECT on disposable tor_ci;
-- it cannot authorize schema changes or prove shared-runner HTTP capacity.
\echo 'PERF04_FTS_DENSE_LOAD_EXPLAIN'
EXPLAIN (ANALYZE, BUFFERS, SETTINGS)
EXECUTE perf04_public_fts('Load', 0, 21, NULL, NULL, NULL, NULL);

\echo 'PERF04_FTS_RARE_9999_EXPLAIN'
EXPLAIN (ANALYZE, BUFFERS, SETTINGS)
EXECUTE perf04_public_fts('9999', 0, 21, NULL, NULL, NULL, NULL);

DEALLOCATE perf04_public_fts;
