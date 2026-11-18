-- A single SELECT shared by read-only export and in-transaction comparison.
WITH source AS (
    SELECT id,bucket FROM public.free_storage_packs WHERE id=:'pack_id'::bigint
), members AS MATERIALIZED (
    SELECT g.chapter_id,g.version_id,g.generation,g.content_sha256,g.size_bytes
      FROM source s JOIN public.free_edition_storage_generations g ON g.pack_id=s.id
     LIMIT 4097
), replacements AS MATERIALIZED (
    SELECT m.chapter_id,m.version_id,m.generation AS source_generation,
           a.generation AS active_generation,g.pack_id,g.frame_offset,g.frame_bytes,
           g.size_bytes,encode(g.content_sha256,'hex') AS content_sha256
      FROM members m
      JOIN public.free_edition_active_storage a USING (chapter_id,version_id)
      JOIN public.free_edition_storage_generations g ON
           (g.chapter_id,g.version_id,g.generation)=(a.chapter_id,a.version_id,a.generation)
      JOIN public.free_storage_packs p ON p.id=g.pack_id
      JOIN public.free_pack_placement_gates gate ON gate.pack_id=p.id
      JOIN source s ON p.bucket=s.bucket AND p.id<>s.id
     WHERE gate.sealed_at IS NULL AND
           (g.content_sha256,g.size_bytes)=(m.content_sha256,m.size_bytes)
), targets AS (
    SELECT p.id AS pack_id,p.bucket,p.object_key AS pack_key,p.manifest_key,
           encode(p.pack_sha256,'hex') AS pack_sha256,p.pack_bytes,
           encode(p.manifest_sha256,'hex') AS manifest_sha256,p.manifest_bytes,p.seek_table_start
      FROM public.free_storage_packs p
      JOIN (SELECT DISTINCT pack_id FROM replacements) r ON r.pack_id=p.id
)
SELECT jsonb_build_object(
    'source_pack_id',(SELECT id FROM source),
    'replacements',(SELECT jsonb_agg(to_jsonb(r) ORDER BY chapter_id,version_id,source_generation)
                      FROM replacements r),
    'packs',(SELECT jsonb_agg(to_jsonb(t) ORDER BY pack_id) FROM targets t)
) AS replacement_plan
WHERE (SELECT count(*) FROM members) BETWEEN 1 AND 4096
  AND (SELECT count(*) FROM replacements)=(SELECT count(*) FROM members)
