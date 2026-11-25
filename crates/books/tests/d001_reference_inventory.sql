-- D-001 schema evidence only: enumerate installed FKs without reading application rows.
-- A missing or unclassified edge is never evidence that any edition can be removed.
-- Soft references and in-flight capabilities are deliberately outside this report.
WITH actual AS (
    SELECT source_ns.nspname::text AS source_schema,
           source.relname::text AS source_table,
           c.conname::text AS constraint_name,
           keys.source_columns,
           keys.target_columns,
           c.convalidated AS validated,
           c.confdeltype::text AS delete_action
      FROM pg_catalog.pg_constraint c
      JOIN pg_catalog.pg_class source ON source.oid=c.conrelid
      JOIN pg_catalog.pg_namespace source_ns ON source_ns.oid=source.relnamespace
     CROSS JOIN LATERAL (
         SELECT array_agg(src.attname::text ORDER BY k.ord) AS source_columns,
                array_agg(dst.attname::text ORDER BY k.ord) AS target_columns
           FROM ROWS FROM (unnest(c.conkey), unnest(c.confkey))
                WITH ORDINALITY AS k(src_num, dst_num, ord)
           JOIN pg_catalog.pg_attribute src
             ON src.attrelid=c.conrelid AND src.attnum=k.src_num
           JOIN pg_catalog.pg_attribute dst
             ON dst.attrelid=c.confrelid AND dst.attnum=k.dst_num
     ) keys
     WHERE c.contype='f'
       AND c.confrelid=pg_catalog.to_regclass('public.chapter_versions')
), reviewed(source_table, source_columns, target_columns, delete_action, classification) AS (
    VALUES
      ('chapter_purchase_facts', ARRAY['chapter_id','version_id'], ARRAY['chapter_id','id'], 'r', 'purchase-pin'),
      ('reader_saved_free_chapters', ARRAY['chapter_id','version_id'], ARRAY['chapter_id','id'], 'r', 'saved-free-pin'),
      ('book_layer_free_archive_versions', ARRAY['version_id'], ARRAY['id'], 'r', 'layer-free-pin'),
      ('reader_free_opened_editions', ARRAY['chapter_id','version_id'], ARRAY['chapter_id','id'], 'c', 'verified-first-open'),
      ('reader_free_open_intents', ARRAY['chapter_id','version_id'], ARRAY['chapter_id','id'], 'c', 'delivery-intent'),
      ('chapter_purchase_commands', ARRAY['chapter_id','expected_version_id'], ARRAY['chapter_id','id'], 'r', 'checkout-replay')
)
SELECT a.source_schema, a.source_table, a.constraint_name,
       a.source_columns, a.target_columns, a.validated, a.delete_action,
       CASE WHEN a.validated THEN coalesce(r.classification, 'unreviewed')
            ELSE 'unreviewed'
       END AS classification
  FROM actual a
  LEFT JOIN reviewed r
    ON a.source_schema='public' AND r.source_table=a.source_table
   AND r.source_columns=a.source_columns AND r.target_columns=a.target_columns
   AND r.delete_action=a.delete_action
 ORDER BY a.source_schema, a.source_table, a.constraint_name
