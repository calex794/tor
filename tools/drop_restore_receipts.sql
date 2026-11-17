-- DROP-08a: run against an isolated restored PG18 snapshot with a read-only
-- role. This exports private names, sizes and immutable receipt digests.
-- psql variables: bucket (one source bucket), prefix (literal key prefix).
\set ON_ERROR_STOP on
BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;
SET LOCAL statement_timeout = '120s';
COPY (
    SELECT object_key, size_bytes, encode(sha256, 'hex')
      FROM (
        SELECT object_key, size_bytes, sha256
          FROM free_chapter_storage_objects WHERE bucket=:'bucket'
        UNION ALL
        SELECT object_key, pack_bytes, pack_sha256
          FROM free_storage_packs WHERE bucket=:'bucket'
        UNION ALL
        SELECT manifest_key, manifest_bytes, manifest_sha256
          FROM free_storage_packs WHERE bucket=:'bucket'
        UNION ALL
        SELECT object_key, size_bytes, sha256_ciphertext
          FROM storage_objects WHERE bucket=:'bucket' AND state='attached'
      ) AS receipts
     WHERE starts_with(object_key, :'prefix')
     ORDER BY object_key COLLATE "C"
) TO STDOUT WITH (FORMAT csv);
COMMIT;
