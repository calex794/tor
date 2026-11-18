-- DROP-07: PostgreSQL reference evidence for one registered immutable PACK.
-- A missing reference does not prove expiry, backup coverage or S3 identity.
\set ON_ERROR_STOP on
BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;
SET LOCAL statement_timeout = '120s';

-- Require a registered PACK and a nonempty bounded set of unique memberships.
SELECT p.id
  FROM free_storage_packs p
 WHERE p.id = :'pack_id'::bigint
   AND (
       SELECT count(*) BETWEEN 1 AND 4096
          AND count(*) = count(DISTINCT (members.chapter_id, members.version_id))
         FROM (
             SELECT g.chapter_id, g.version_id
               FROM free_edition_storage_generations g
              WHERE g.pack_id = p.id
              LIMIT 4097
         ) members
   )
\gset source_

COPY (
    WITH source AS (
        SELECT p.*,
               EXISTS (
                   SELECT 1 FROM free_pack_jobs j
                   JOIN free_pack_activation_jobs a ON a.pack_job_id = j.id
                    WHERE j.pack_id = p.id AND a.status IN ('pending','running')
               ) OR EXISTS (
                   SELECT 1 FROM free_pack_direct_import_stages s
                   JOIN chapter_import_jobs j ON j.id = s.job_id
                    WHERE s.bucket = p.bucket AND s.object_key = p.object_key
                      AND j.status IN ('queued','running')
               ) AS pending_source
          FROM free_storage_packs p
         WHERE p.id = :'source_id'::bigint
    ), members AS MATERIALIZED (
        SELECT g.*
          FROM free_edition_storage_generations g
         WHERE g.pack_id = :'source_id'::bigint
    ), saved_versions AS (
        SELECT DISTINCT saved.chapter_id, saved.version_id
          FROM reader_saved_free_chapters saved
          JOIN members g USING (chapter_id, version_id)
    ), evidence AS (
        SELECT p.id AS pack_id,
               encode(p.pack_sha256, 'hex') AS pack_sha256,
               encode(p.manifest_sha256, 'hex') AS manifest_sha256,
               g.frame_offset, g.frame_bytes, g.chapter_id, g.version_id,
               g.generation, g.size_bytes, encode(g.content_sha256, 'hex') AS content_sha256,
               EXISTS (
                   SELECT 1 FROM free_edition_active_storage a
                    WHERE (a.chapter_id, a.version_id, a.generation) =
                          (g.chapter_id, g.version_id, g.generation)
               ) AS active_here,
               EXISTS (
                   SELECT 1 FROM chapter_publications published
                    WHERE (published.chapter_id, published.version_id) =
                          (g.chapter_id, g.version_id)
               ) AS current_publication,
               saved.version_id IS NOT NULL AS saved_free,
               EXISTS (
                   SELECT 1 FROM book_layer_free_archive_versions layer
                    WHERE (layer.book_id, layer.chapter_id, layer.version_id) =
                          (c.book_id, g.chapter_id, g.version_id)
               ) AS layer_archive,
               (b.admin_hold OR c.admin_hold) AS admin_hold,
               p.pending_source OR EXISTS (
                   SELECT 1 FROM free_pack_job_members m
                   JOIN free_pack_jobs j ON j.id = m.job_id
                    WHERE (m.chapter_id, m.version_id) = (g.chapter_id, g.version_id)
                      AND j.status IN ('pending','running')
               ) AS pending_pack_work
          FROM source p
          JOIN members g ON g.pack_id = p.id
          JOIN chapters c ON c.id = g.chapter_id
          JOIN books b ON b.id = c.book_id
          LEFT JOIN saved_versions saved ON (saved.chapter_id, saved.version_id) =
                                             (g.chapter_id, g.version_id)
    )
    SELECT evidence.*,
           CASE WHEN active_here OR current_publication OR saved_free
                      OR layer_archive OR admin_hold OR pending_pack_work
                THEN 'RETAIN' ELSE 'UNKNOWN' END AS retention
      FROM evidence
     ORDER BY frame_offset
) TO STDOUT WITH (FORMAT csv, HEADER true);
COMMIT;
