-- DROP-08b: export historical FREE pin source coverage from a restored PG18.
-- Read-only, includes archived Layer editions and reader-saved FREE editions.
-- A physical receipt is still checked against an independent object manifest.
\set ON_ERROR_STOP on
BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;
SET LOCAL statement_timeout = '120s';
COPY (
    WITH pinned AS (
        SELECT 'layer'::text AS kind, book_id, chapter_id, version_id
          FROM book_layer_free_archive_versions
        UNION
        SELECT 'saved'::text AS kind, NULL::bigint AS book_id, chapter_id, version_id
          FROM reader_saved_free_chapters
    )
    SELECT p.kind, p.chapter_id, p.version_id,
           CASE WHEN v.id IS NOT NULL AND c.id IS NOT NULL
                     AND (p.kind='saved' OR p.book_id=c.book_id)
                THEN 1 ELSE 0 END AS valid_member,
           CASE WHEN v.body_markdown IS NOT NULL THEN 1 ELSE 0 END AS pg_body,
           CASE WHEN EXISTS (
               SELECT 1 FROM free_chapter_storage_objects s
                WHERE s.chapter_id=p.chapter_id AND s.version_id=p.version_id
                  AND s.sha256=v.content_sha256
           ) THEN 1 ELSE 0 END AS single_receipt,
           CASE WHEN EXISTS (
               SELECT 1 FROM free_edition_active_storage a
               JOIN free_edition_storage_generations g
                 ON g.chapter_id=a.chapter_id AND g.version_id=a.version_id
                AND g.generation=a.generation AND g.placement_kind='pack'
               JOIN free_storage_packs pack ON pack.id=g.pack_id
                WHERE a.chapter_id=p.chapter_id AND a.version_id=p.version_id
                  AND g.content_sha256=v.content_sha256
           ) THEN 1 ELSE 0 END AS pack_receipt
      FROM pinned p
      LEFT JOIN chapter_versions v
        ON v.chapter_id=p.chapter_id AND v.id=p.version_id
      LEFT JOIN chapters c ON c.id=p.chapter_id
     ORDER BY p.kind COLLATE "C", p.chapter_id, p.version_id
) TO STDOUT WITH (FORMAT csv);
COMMIT;
