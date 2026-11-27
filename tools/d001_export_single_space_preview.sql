-- D-001: read-only snapshot of prospective FREE SINGLE retirement.
-- Does not record observations, lock versions, close gates or authorize DELETE.
-- psql variables: bucket (one source bucket), prefix (literal FREE SINGLE prefix).
\set ON_ERROR_STOP on
BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;
SET LOCAL statement_timeout = '120s';
SET LOCAL lock_timeout = '1s';
-- Refuse malformed or overly broad scopes rather than report a false zero.
SELECT CAST(CASE
    WHEN :'bucket' ~ '^[a-z][a-z0-9-]{1,61}[a-z0-9]$'
        AND :'bucket' NOT LIKE 'public%'
        AND :'prefix' ~ '^private/free-chapters/([1-9][0-9]{0,18}(/([1-9][0-9]{0,18})?)?)?$'
    THEN '1' ELSE 'invalid D001 preview scope' END AS INTEGER) AS scope_valid \gset
COPY (
    SELECT f.object_key, f.size_bytes, encode(f.sha256, 'hex')
      FROM d001_free_single_retirement_preflight f
     WHERE f.bucket=:'bucket' AND starts_with(f.object_key, :'prefix')
       AND NOT EXISTS (
           SELECT 1 FROM d001_free_single_delivery_holds h
            WHERE h.chapter_id=f.chapter_id AND h.version_id=f.version_id
              AND h.protected_until>statement_timestamp()
       )
     ORDER BY f.object_key COLLATE "C"
) TO STDOUT WITH (FORMAT csv);
COMMIT;
