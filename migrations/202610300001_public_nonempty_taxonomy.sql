-- PERF-233: optional fast path for sparse public taxonomy.
-- One small btree covers visible books with at least one nonempty facet.
-- No array payload is stored inside the index, avoiding oversized tuples.
-- PostgreSQL MVCC maintains eligibility at commit: hiding or holding a book
-- removes its metadata immediately from live reads.
--
-- Dense catalogs use the original three parallel sequential scans instead
-- of walking this index through most heap rows. Query selection consults
-- pg_class statistics only for performance; it NEVER changes access rules.
CREATE INDEX books_public_nonempty_taxonomy_idx
    ON books (id)
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
      AND (
          genres <> '{}'::text[]
          OR tags <> '{}'::text[]
          OR fandoms <> '{}'::text[]
      );
