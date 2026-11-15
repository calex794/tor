-- I-reviewed F projection for current genre/tag membership. No new collection,
-- personal state, coverage or backfill; C reuses its bounded book observations.
CREATE OR REPLACE VIEW reader_return_public_books WITH (security_barrier=true) AS
SELECT id,title,genres,tags FROM rec_public_books;
COMMENT ON VIEW reader_return_public_books IS
    'F-approved read-only C report port: current public/basic/general book visibility and genre/tag membership from rec_public_books; no duplicated visibility policy.';
