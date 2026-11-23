-- C-08 / D-001, 2026-11-23: exact opened FREE edition is not a book bookmark.
-- No backfill from aggregate book_chapter_open_readers: the old row has no version_id.
-- Open evidence is not a separate entitlement: hard/legal chapter deletion may
-- cascade it, while actual saved FREE pins retain their own RESTRICT FK.
-- First successful version-specific open is immutable and preserves the physical
-- edition even before the actor bookmarks the Book; this is a retention proof,
-- not an independent ACL or paid entitlement. No S3 DELETE/GC authorization.
CREATE TABLE reader_free_opened_editions (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL,
    first_opened_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY(user_id,chapter_id),
    CONSTRAINT reader_free_opened_exact_version_fk
      FOREIGN KEY (chapter_id,version_id)
      REFERENCES chapter_versions(chapter_id,id) ON DELETE CASCADE
);
CREATE INDEX c08_opened_exact_version_idx
    ON reader_free_opened_editions(chapter_id,version_id);
COMMENT ON TABLE reader_free_opened_editions IS
    'Exact first verified authenticated FREE chapter open; not a standalone entitlement and never inferred from book bookmark or aggregate opens.';

-- Preserve the full existing D-001 view schema and append only has_opened_free.
-- An open that is not yet bookmarked is logically retained as historical proof,
-- while reader_retained remains reserved for real pinned rights.
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
           OR identity.has_opened_free
       ) AS is_logically_retained,
       anchors.has_discussion_anchor,
       anchors.has_text_issue_anchor,
       anchors.has_cursor_reference,
       anchors.has_idempotency_reference,
       anchors.has_first_release_fact,
       identity.has_opened_free
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
           ) AS has_layer_free_archive,
           EXISTS (
               SELECT 1 FROM reader_free_opened_editions opened
               WHERE opened.chapter_id=v.chapter_id AND opened.version_id=v.id
           ) AS has_opened_free
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
    'D-001: current, working, schedule, reader pins, versioned first opens and historical anchors are logical retention proofs. This is NOT an ACL or physical S3 deletion authorization.';
