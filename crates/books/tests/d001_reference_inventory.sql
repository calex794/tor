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
), reviewed(source_table, constraint_name, source_columns, target_columns,
            delete_action, classification) AS (
    -- These labels describe the reference, not its current status or lifetime.
    VALUES
      ('book_layer_free_archive_versions',
       'book_layer_free_archive_versions_version_id_fkey',
       ARRAY['version_id'], ARRAY['id'], 'r', 'layer-free-pin'),
      ('chapter_encrypted_release_events',
       'chapter_encrypted_release_events_chapter_id_version_id_fkey',
       ARRAY['chapter_id','version_id'], ARRAY['chapter_id','id'], 'r', 'verified-encrypted-release'),
      ('chapter_first_release_events',
       'chapter_first_release_events_version_id_fkey',
       ARRAY['version_id'], ARRAY['id'], 'r', 'first-release-metadata'),
      ('chapter_publications',
       'chapter_publications_chapter_id_version_id_fkey',
       ARRAY['chapter_id','version_id'], ARRAY['chapter_id','id'], 'a', 'published-head'),
      ('chapter_purchase_commands',
       'chapter_purchase_commands_chapter_id_expected_version_id_fkey',
       ARRAY['chapter_id','expected_version_id'], ARRAY['chapter_id','id'], 'r', 'checkout-replay'),
      ('chapter_purchase_facts',
       'chapter_purchase_facts_chapter_id_version_id_fkey',
       ARRAY['chapter_id','version_id'], ARRAY['chapter_id','id'], 'r', 'purchase-pin'),
      ('chapter_reader_manifests',
       'chapter_reader_manifests_chapter_id_version_id_fkey',
       ARRAY['chapter_id','version_id'], ARRAY['chapter_id','id'], 'r', 'verified-reader-manifest'),
      ('chapter_save_commands',
       'chapter_save_commands_version_id_fkey',
       ARRAY['version_id'], ARRAY['id'], 'r', 'author-save-replay'),
      ('chapter_scheduled_releases',
       'chapter_scheduled_releases_version_id_fkey',
       ARRAY['version_id'], ARRAY['id'], 'r', 'scheduled-release-reference'),
      ('chapter_version_assets',
       'chapter_version_assets_chapter_id_version_id_fkey',
       ARRAY['chapter_id','version_id'], ARRAY['chapter_id','id'], 'r', 'exact-paid-asset-reference'),
      ('chapter_version_content_shapes',
       'chapter_version_content_shapes_chapter_id_version_id_fkey',
       ARRAY['chapter_id','version_id'], ARRAY['chapter_id','id'], 'r', 'derived-content-coordinates'),
      ('chapter_working_drafts',
       'chapter_working_drafts_chapter_id_version_id_fkey',
       ARRAY['chapter_id','version_id'], ARRAY['chapter_id','id'], 'r', 'author-working-draft'),
      ('discussion_anchors',
       'discussion_anchors_chapter_id_version_id_fkey',
       ARRAY['chapter_id','version_id'], ARRAY['chapter_id','id'], 'r', 'historical-discussion-anchor'),
      ('discussion_comments',
       'discussion_comments_chapter_id_version_id_fkey',
       ARRAY['chapter_id','version_id'], ARRAY['chapter_id','id'], 'r', 'historical-discussion-anchor'),
      ('finance_direct_swap_commands',
       'finance_direct_swap_commands_target_chapter_id_target_vers_fkey',
       ARRAY['target_chapter_id','target_version_id'], ARRAY['chapter_id','id'], 'r', 'swap-command-target'),
      ('free_chapter_storage_objects',
       'free_chapter_storage_version_fk',
       ARRAY['chapter_id','version_id'], ARRAY['chapter_id','id'], 'r', 'free-single-receipt'),
      ('free_edition_storage_generations',
       'free_edition_storage_generations_chapter_id_version_id_fkey',
       ARRAY['chapter_id','version_id'], ARRAY['chapter_id','id'], 'r', 'free-placement-generation'),
      ('free_pack_job_members',
       'free_pack_job_members_chapter_id_version_id_fkey',
       ARRAY['chapter_id','version_id'], ARRAY['chapter_id','id'], 'r', 'free-pack-job-input'),
      ('reader_book_positions',
       'reader_book_positions_chapter_id_version_id_fkey',
       ARRAY['chapter_id','version_id'], ARRAY['chapter_id','id'], 'a', 'reader-cursor'),
      ('reader_chapter_completions',
       'reader_chapter_completions_chapter_id_first_version_id_fkey',
       ARRAY['chapter_id','first_version_id'], ARRAY['chapter_id','id'], 'a', 'first-completion-metadata'),
      ('reader_chapter_starts',
       'reader_chapter_starts_chapter_id_first_version_id_fkey',
       ARRAY['chapter_id','first_version_id'], ARRAY['chapter_id','id'], 'a', 'first-start-metadata'),
      ('reader_free_open_intents',
       'c08_free_open_intent_exact_version_fk',
       ARRAY['chapter_id','version_id'], ARRAY['chapter_id','id'], 'c', 'delivery-intent'),
      ('reader_free_opened_editions',
       'reader_free_opened_exact_version_fk',
       ARRAY['chapter_id','version_id'], ARRAY['chapter_id','id'], 'c', 'verified-first-open'),
      ('reader_saved_free_chapters',
       'reader_saved_free_chapter_version_fk',
       ARRAY['chapter_id','version_id'], ARRAY['chapter_id','id'], 'r', 'saved-free-pin'),
      ('social_free_comment_reports',
       'social_free_comment_reports_chapter_id_version_id_fkey',
       ARRAY['chapter_id','version_id'], ARRAY['chapter_id','id'], 'r', 'moderation-discussion-evidence'),
      ('social_paid_comment_reports',
       'social_paid_comment_reports_chapter_id_version_id_fkey',
       ARRAY['chapter_id','version_id'], ARRAY['chapter_id','id'], 'r', 'moderation-discussion-evidence'),
      ('text_issue_status_events',
       'text_issue_status_events_resolved_version_id_fkey',
       ARRAY['resolved_version_id'], ARRAY['id'], 'r', 'historical-correction-anchor'),
      ('text_issues',
       'text_issues_chapter_id_resolved_version_id_fkey',
       ARRAY['chapter_id','resolved_version_id'], ARRAY['chapter_id','id'], 'r', 'historical-correction-anchor'),
      ('text_issues',
       'text_issues_chapter_id_version_id_fkey',
       ARRAY['chapter_id','version_id'], ARRAY['chapter_id','id'], 'r', 'historical-correction-anchor')
)
SELECT a.source_schema, a.source_table, a.constraint_name,
       a.source_columns, a.target_columns, a.validated, a.delete_action,
       CASE WHEN a.validated THEN coalesce(r.classification, 'unreviewed')
            ELSE 'unreviewed'
       END AS classification
  FROM actual a
  LEFT JOIN reviewed r
    ON a.source_schema='public' AND r.source_table=a.source_table
   AND r.constraint_name=a.constraint_name
   AND r.source_columns=a.source_columns AND r.target_columns=a.target_columns
   AND r.delete_action=a.delete_action
 ORDER BY a.source_schema, a.source_table, a.constraint_name
