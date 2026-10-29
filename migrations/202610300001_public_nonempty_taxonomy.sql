-- PERF-233: avoid three full scans of books when facets are sparse.
-- Partial indexes track each public facet independently, preserving
-- immediate visibility/hold/delete changes through PostgreSQL MVCC.
-- Store only book IDs: taxonomy text arrays can exceed btree tuple limits.
--
-- On dense catalogs PostgreSQL remains free to choose a cheaper sequential
-- scan instead of an expensive heap-fetching partial index scan.
CREATE INDEX books_public_nonempty_genres_idx
    ON books (id)
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
      AND cardinality(genres) > 0;

CREATE INDEX books_public_nonempty_tags_idx
    ON books (id)
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
      AND cardinality(tags) > 0;

CREATE INDEX books_public_nonempty_fandoms_idx
    ON books (id)
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
      AND cardinality(fandoms) > 0;
