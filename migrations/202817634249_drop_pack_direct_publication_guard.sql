-- DROP-04b: extend the original S349 publication invariant, do not remove it.
-- A bodyless free edition may be published with either its exact immutable
-- SINGLE receipt OR an exact independently verified, staged direct PACK
-- whose per-edition active placement was created in the same PG transaction.
-- This migration is additive and MUST NOT run on the live VPS.
CREATE OR REPLACE FUNCTION guard_s349_free_publication()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM chapter_versions v
        JOIN chapters c ON c.id=v.chapter_id
        WHERE v.id=NEW.version_id AND v.chapter_id=NEW.chapter_id
          AND c.access_level='free' AND v.body_markdown IS NULL
          AND NOT EXISTS (
              SELECT 1 FROM free_chapter_storage_objects f
              WHERE f.chapter_id=v.chapter_id AND f.version_id=v.id
          )
          AND NOT EXISTS (
              SELECT 1
              FROM free_edition_active_storage a
              JOIN free_edition_storage_generations g
                ON g.chapter_id=a.chapter_id AND g.version_id=a.version_id
               AND g.generation=a.generation AND g.placement_kind='pack'
               AND g.content_sha256=v.content_sha256
              JOIN free_storage_packs pack ON pack.id=g.pack_id
              JOIN free_pack_direct_import_stages stage
                ON stage.bucket=pack.bucket
               AND stage.object_key=pack.object_key
               AND stage.pack_sha256=pack.pack_sha256
               AND stage.manifest_sha256=pack.manifest_sha256
              WHERE a.chapter_id=v.chapter_id AND a.version_id=v.id
                AND stage.book_id=c.book_id
          )
    ) THEN
        RAISE EXCEPTION
          'S349/DROP: free S3 publication requires exact SINGLE or verified active PACK'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;

COMMENT ON FUNCTION guard_s349_free_publication() IS
    'S349 + DROP-04b: bodyless free publication requires exact immutable SINGLE or staged verified active PACK, never an unattached object.';
