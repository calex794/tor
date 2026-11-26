-- D-001: select one exact registered bodyless FREE SINGLE receipt.
-- Isolated restored PG18, SELECT-only role; never infer identity from a SHA.
-- psql variables: bucket, chapter_id, version_id.
\set ON_ERROR_STOP on
BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;
SET LOCAL statement_timeout = '120s';
COPY (
    SELECT f.object_key, f.size_bytes, encode(f.sha256, 'hex')
      FROM free_chapter_storage_objects f
      JOIN chapter_versions v ON v.chapter_id=f.chapter_id AND v.id=f.version_id
     WHERE f.bucket=:'bucket'
       AND f.chapter_id=:'chapter_id'::bigint AND f.version_id=:'version_id'::bigint
       AND v.body_markdown IS NULL AND v.storage_object_id IS NULL
       AND v.content_sha256=f.sha256
     ORDER BY f.object_key COLLATE "C"
) TO STDOUT WITH (FORMAT csv);
COMMIT;
