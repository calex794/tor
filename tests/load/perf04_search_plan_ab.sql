-- PERF-04b: paired PostgreSQL 18 EXPLAIN on disposable tor_ci only.
-- The same prepared SELECT is executed with default planner settings and
-- with bitmap + sequential scans disabled so that ordered index access can be
-- compared on 10k and 100k data sets. This file contains NO DDL/DML.
-- These settings are SET LOCAL in a READ ONLY transaction and rolled back.
\set ON_ERROR_STOP on
\echo PERF04_AB_TOR_CI_IDENTITY
SELECT current_database() AS db, current_user AS role, version() AS pg;

BEGIN READ ONLY;
SET LOCAL statement_timeout = '15s';

PREPARE perf04_ab_public_fts(text, bigint, bigint, text, text, text, text) AS
SELECT id,title,target_language,author_name,genres,tags,fandoms FROM books
WHERE visibility='public' AND visibility_status='active' AND access_level='basic' AND lifecycle_status<>'pending_deletion' AND NOT admin_hold AND NOT is_deleted
  AND ($1::text = '' OR
       (to_tsvector('simple'::regconfig, translate(lower(normalize(title, NFKC)), 'ё', 'е')) || to_tsvector('simple'::regconfig, translate(lower(normalize(author_name, NFKC)), 'ё', 'е')))
           @@ plainto_tsquery('simple'::regconfig, translate(lower(normalize($1::text, NFKC)), 'ё', 'е')))
  AND id > $2
  AND ($4::text IS NULL OR target_language = $4)
  AND ($5::text IS NULL OR genres @> ARRAY[$5::text])
  AND ($6::text IS NULL OR tags @> ARRAY[$6::text])
  AND ($7::text IS NULL OR fandoms @> ARRAY[$7::text])
ORDER BY id ASC LIMIT $3;

\echo PERF04_AB_DEFAULT_DENSE_LOAD
EXPLAIN (ANALYZE, BUFFERS, SETTINGS)
EXECUTE perf04_ab_public_fts('Load', 0, 21, NULL, NULL, NULL, NULL);

\echo PERF04_AB_DEFAULT_RARE_9999
EXPLAIN (ANALYZE, BUFFERS, SETTINGS)
EXECUTE perf04_ab_public_fts('9999', 0, 21, NULL, NULL, NULL, NULL);

-- Planner experiment ONLY. Never apply these flags in production.
-- An index-ordered scan may help dense terms but be much worse for rare terms.
SET LOCAL enable_bitmapscan = off;
SET LOCAL enable_seqscan = off;

\echo PERF04_AB_ORDERED_DENSE_LOAD
EXPLAIN (ANALYZE, BUFFERS, SETTINGS)
EXECUTE perf04_ab_public_fts('Load', 0, 21, NULL, NULL, NULL, NULL);

\echo PERF04_AB_ORDERED_RARE_9999
EXPLAIN (ANALYZE, BUFFERS, SETTINGS)
EXECUTE perf04_ab_public_fts('9999', 0, 21, NULL, NULL, NULL, NULL);

ROLLBACK;
DEALLOCATE perf04_ab_public_fts;
