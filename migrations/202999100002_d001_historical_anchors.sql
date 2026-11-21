-- D-001 follow-up: historical text anchors are retention obligations,
-- not purchase grants. Comments/corrections may refer to an older exact
-- immutable edition that nobody purchased or explicitly saved.
-- Version-bound references must remain available for moderation and
-- discussion even though no new reader entitlement is created.
--
-- Additive migration: retain all existing D-001 view columns/order and keep
-- the physical deletion path DISABLED.

CREATE INDEX d001_discussion_version_anchor_idx
    ON discussion_comments(chapter_id,version_id)
    WHERE chapter_id IS NOT NULL AND version_id IS NOT NULL;

CREATE INDEX d001_text_issue_version_anchor_idx
    ON text_issues(chapter_id,version_id);

CREATE VIEW d001_chapter_historical_anchors AS
SELECT v.chapter_id,
       v.id AS version_id,
       EXISTS (
           SELECT 1 FROM discussion_comments comments
           WHERE comments.chapter_id=v.chapter_id
             AND comments.version_id=v.id
       ) OR EXISTS (
           SELECT 1 FROM discussion_anchors anchors
           WHERE anchors.chapter_id=v.chapter_id
             AND anchors.version_id=v.id
       ) AS has_discussion_anchor,
       EXISTS (
           SELECT 1 FROM text_issues issues
           WHERE issues.chapter_id=v.chapter_id
             AND (issues.version_id=v.id OR issues.resolved_version_id=v.id)
       ) OR EXISTS (
           SELECT 1 FROM text_issue_status_events events
           WHERE events.resolved_version_id=v.id
       ) AS has_text_issue_anchor,
       EXISTS (
           SELECT 1 FROM reader_book_positions position
           WHERE position.chapter_id=v.chapter_id
             AND position.version_id=v.id
       ) AS has_cursor_reference,
       EXISTS (
           SELECT 1 FROM chapter_save_commands command
           WHERE command.chapter_id=v.chapter_id
             AND command.version_id=v.id
       ) AS has_idempotency_reference,
       EXISTS (
           SELECT 1 FROM chapter_first_release_events event
           WHERE event.chapter_id=v.chapter_id
             AND event.version_id=v.id
       ) AS has_first_release_fact
FROM chapter_versions v;

COMMENT ON VIEW d001_chapter_historical_anchors IS
    'D-001: exact discussion and correction anchors protect old text; cursor, idempotency, release facts preserve metadata but are not reader entitlements. Never authorize GC from this view.';

CREATE OR REPLACE VIEW d001_chapter_materialization_roles AS
SELECT identity.chapter_id,
       identity.version_id,
       identity.is_published_head,
       identity.is_author_working,
       identity.is_pending_schedule,
       identity.has_direct_purchase,
       identity.has_saved_free,
       identity.has_layer_free_archive,
       (
           identity.is_published_head
           OR identity.has_direct_purchase
           OR identity.has_saved_free
           OR identity.has_layer_free_archive
       ) AS is_reader_retained,
       (
           identity.is_published_head
           OR identity.is_author_working
           OR identity.is_pending_schedule
           OR identity.has_direct_purchase
           OR identity.has_saved_free
           OR identity.has_layer_free_archive
           OR anchors.has_discussion_anchor
           OR anchors.has_text_issue_anchor
       ) AS is_logically_retained,
       anchors.has_discussion_anchor,
       anchors.has_text_issue_anchor,
       anchors.has_cursor_reference,
       anchors.has_idempotency_reference,
       anchors.has_first_release_fact
FROM (
    SELECT v.chapter_id,
           v.id AS version_id,
           COALESCE(p.version_id = v.id, FALSE) AS is_published_head,
           COALESCE(d.version_id = v.id, FALSE) AS is_author_working,
           COALESCE(
               scheduled.version_id = v.id
               AND scheduled.status IN ('pending', 'claimed'),
               FALSE
           ) AS is_pending_schedule,
           EXISTS (
               SELECT 1 FROM chapter_purchase_facts purchased
               WHERE purchased.chapter_id = v.chapter_id
                 AND purchased.version_id = v.id
           ) AS has_direct_purchase,
           EXISTS (
               SELECT 1 FROM reader_saved_free_chapters saved
               WHERE saved.chapter_id = v.chapter_id
                 AND saved.version_id = v.id
           ) AS has_saved_free,
           EXISTS (
               SELECT 1 FROM book_layer_free_archive_versions archived
               WHERE archived.chapter_id = v.chapter_id
                 AND archived.version_id = v.id
           ) AS has_layer_free_archive
    FROM chapter_versions v
    LEFT JOIN chapter_publications p ON p.chapter_id = v.chapter_id
    LEFT JOIN chapter_working_drafts d ON d.chapter_id = v.chapter_id
    LEFT JOIN chapter_scheduled_releases scheduled
      ON scheduled.chapter_id = v.chapter_id
) identity
JOIN d001_chapter_historical_anchors anchors
  ON anchors.chapter_id=identity.chapter_id
 AND anchors.version_id=identity.version_id;

COMMENT ON VIEW d001_chapter_materialization_roles IS
    'D-001: current, working, scheduled, exact reader rights and historical discussion/issue anchors determine logical retention. Not ACL, physical deletion or object-store GC authorization.';

-- Read-only *registered* byte overhang. This is the gap between logical
-- retention and immutable S3 receipts, not permission to reclaim storage.
-- A FREE SINGLE may also have been copied into an active PACK; billing,
-- PACK utilization, offsite DR and open signed URLs are not represented.
CREATE VIEW d001_registered_storage_overhang AS
SELECT v.chapter_id,
       v.id AS version_id,
       v.version_no,
       f.object_key AS free_single_key,
       f.size_bytes AS free_single_registered_bytes,
       paid.object_key AS paid_ciphertext_key,
       paid.size_bytes AS paid_ciphertext_registered_bytes,
       roles.has_cursor_reference,
       roles.has_idempotency_reference,
       roles.has_first_release_fact,
       EXISTS (
           SELECT 1 FROM free_edition_active_storage active
           JOIN free_edition_storage_generations generation
             ON generation.chapter_id=active.chapter_id
            AND generation.version_id=active.version_id
            AND generation.generation=active.generation
           WHERE active.chapter_id=v.chapter_id
             AND active.version_id=v.id
             AND generation.placement_kind='pack'
       ) AS has_active_pack_placement
FROM chapter_versions v
JOIN d001_chapter_materialization_roles roles
  ON roles.chapter_id=v.chapter_id AND roles.version_id=v.id
LEFT JOIN free_chapter_storage_objects f
  ON f.chapter_id=v.chapter_id AND f.version_id=v.id
LEFT JOIN storage_objects paid ON paid.id=v.storage_object_id
WHERE NOT roles.is_logically_retained;

COMMENT ON VIEW d001_registered_storage_overhang IS
    'Read-only D-001 obsolete immutable receipts and bytes. Does not prove physical S3 bytes exist, are reclaimable, have DR copies, or lack in-flight grants; never use as DELETE candidate without additional fencing.';

CREATE VIEW d001_registered_overhang_by_chapter AS
SELECT c.id AS chapter_id,
       count(overhang.version_id) AS superseded_unpinned_materializations,
       COALESCE(sum(overhang.free_single_registered_bytes),0)::bigint
           AS registered_free_single_bytes,
       COALESCE(sum(overhang.paid_ciphertext_registered_bytes),0)::bigint
           AS registered_paid_ciphertext_bytes,
       count(overhang.version_id) FILTER (
           WHERE overhang.has_active_pack_placement
       ) AS also_in_active_pack
FROM chapters c
LEFT JOIN d001_registered_storage_overhang overhang
  ON overhang.chapter_id=c.id
GROUP BY c.id;

COMMENT ON VIEW d001_registered_overhang_by_chapter IS
    'Inventory of exact unpinned version receipts, not a storage bill nor GC eligibility; counts exclude preserved discussion and correction anchors.';
