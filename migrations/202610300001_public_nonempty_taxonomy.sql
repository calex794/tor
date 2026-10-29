-- PERF-233: avoid three full scans of books when facets are sparse.
-- Partial indexes track each public facet independently, preserving
-- immediate visibility/hold/delete changes through PostgreSQL MVCC.
-- Store only book IDs: taxonomy text arrays can exceed btree tuple limits.
--
-- Array equality/inequality is supported by column MCV statistics, unlike
-- cardinality(...) > 0 (which can underestimate dense rows by 3x).
-- The planner can therefore distinguish sparse and dense facets.
CREATE INDEX books_public_nonempty_genres_idx
    ON books (id)
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
      AND genres <> '{}'::text[];

CREATE INDEX books_public_nonempty_tags_idx
    ON books (id)
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
      AND tags <> '{}'::text[];

CREATE INDEX books_public_nonempty_fandoms_idx
    ON books (id)
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
      AND fandoms <> '{}'::text[];
