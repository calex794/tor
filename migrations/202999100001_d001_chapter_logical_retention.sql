-- D-001 / 2026-11-21: retained logical editions are not immutable S3 materials.
--
-- No table that encodes rights, facts, object identity or idempotent save
-- receipts is mutated here. Physical chapter_versions and their S3 keys stay
-- immutable; old unpinned materializations are *not* permanent logical
-- editions. Never use this view alone as permission to DELETE an S3 object:
-- in-flight capabilities, progress, object placements and independent DR
-- require a different, fenced GC contract (currently disabled).
--
-- Direct purchases are historical immutable facts, even after revocation.
-- Saved free pins are explicit C rights. Layer free archives preserve the
-- exact frozen edition for ordinary deletion. Paid Layer membership currently
-- resolves the live published chapter unless another exact pin applies.

CREATE INDEX d001_direct_purchase_version_pin_idx
    ON chapter_purchase_facts (chapter_id, version_id);

CREATE INDEX d001_saved_free_version_pin_idx
    ON reader_saved_free_chapters (chapter_id, version_id);

CREATE INDEX d001_layer_archive_version_pin_idx
    ON book_layer_free_archive_versions (chapter_id, version_id);

CREATE VIEW d001_chapter_materialization_roles AS
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
       ) AS is_logically_retained
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
) identity;

COMMENT ON VIEW d001_chapter_materialization_roles IS
    'D-001: live current/draft/schedule and exact historical pins determine logical retention. Not an ACL, GC claim, physical deletion authorization or replacement for immutable storage receipts.';

CREATE VIEW d001_chapter_logical_counts AS
SELECT c.id AS chapter_id,
       count(material.version_id)
           FILTER (WHERE material.is_logically_retained) AS logical_editions,
       count(material.version_id)
           FILTER (WHERE material.is_reader_retained) AS reader_retained_editions,
       count(material.version_id) AS physical_materializations,
       count(material.version_id)
           FILTER (WHERE NOT material.is_logically_retained)
           AS superseded_unpinned_materializations
FROM chapters c
LEFT JOIN d001_chapter_materialization_roles material
  ON material.chapter_id = c.id
GROUP BY c.id;

COMMENT ON VIEW d001_chapter_logical_counts IS
    'D-001 diagnostic counts only: physical_materializations includes immutable historical rows; superseded_unpinned is not a list of deletable S3 keys.';
