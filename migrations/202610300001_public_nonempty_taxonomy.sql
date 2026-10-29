-- PERF-233: avoid scanning every published book on each first catalog page
-- when most books have no taxonomy. The predicate is deliberately identical
-- to the public catalog visibility contract: private, held and deleted rows
-- cannot appear in the index.
--
-- An index of only IDs avoids putting unbounded text[] payloads into btree
-- tuples. PostgreSQL fetches taxonomy values from eligible heap rows.
-- Publication, revocation and metadata updates maintain this index
-- transactionally; no TTL cache or asynchronously refreshed public projection.
CREATE INDEX books_public_nonempty_taxonomy_idx
    ON books (id)
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
      AND (
          cardinality(genres) > 0
          OR cardinality(tags) > 0
          OR cardinality(fandoms) > 0
      );
