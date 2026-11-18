-- DROP-07: the two exact object receipts for one registered PACK.
-- psql variables: bucket (source bucket), pack_id (registered PACK id).
\set ON_ERROR_STOP on
BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;
SET LOCAL statement_timeout = '120s';
COPY (
    SELECT receipt.key, receipt.size, encode(receipt.sha256, 'hex')
      FROM free_storage_packs pack
     CROSS JOIN LATERAL (
         VALUES (pack.object_key, pack.pack_bytes, pack.pack_sha256),
                (pack.manifest_key, pack.manifest_bytes, pack.manifest_sha256)
     ) AS receipt(key, size, sha256)
     WHERE pack.bucket = :'bucket' AND pack.id = :'pack_id'::bigint
     ORDER BY receipt.key COLLATE "C"
) TO STDOUT WITH (FORMAT csv);
COMMIT;
