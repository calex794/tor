-- C-16: reader-owned acknowledgement of new chapters of explicitly followed books.
-- Never materialize paid content or create reading/purchase entitlements here.
-- Read state is kept separately from both book follows and saved library rows.
CREATE TABLE reader_follow_notification_reads (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    chapter_id BIGINT NOT NULL REFERENCES chapters(id) ON DELETE CASCADE,
    read_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, chapter_id)
);

-- Single canonical visibility projection for the follow feed and its read-state
-- actions. Derived on demand: no fan-out truncation, worker, or duplicate delivery.
-- Release timestamps are immutable first-release facts, not later edits.
CREATE VIEW reader_followed_release_candidates AS
SELECT follow.user_id,
       book.id AS book_id,
       book.title AS book_title,
       chapter.id AS chapter_id,
       chapter.title AS chapter_title,
       chapter.chapter_no AS chapter_number,
       chapter.access_level,
       chapter.price_amount AS price_minor,
       release.created_at AS released_at,
       (EXTRACT(EPOCH FROM release.created_at) * 1000000)::bigint AS released_micros
FROM reader_book_follows AS follow
JOIN books AS book ON book.id=follow.book_id
JOIN chapters AS chapter ON chapter.book_id=book.id
JOIN chapter_first_release_events AS release ON release.chapter_id=chapter.id
JOIN chapter_publications AS publication ON publication.chapter_id=chapter.id
WHERE release.created_at>=follow.created_at
  AND book.visibility='public'
  AND NOT book.admin_hold AND NOT book.is_deleted
  AND chapter.visibility='public'
  AND NOT chapter.admin_hold AND NOT chapter.is_deleted
  AND (chapter.reader_released_at IS NULL OR chapter.reader_released_at<=now())
  AND (
      (chapter.access_level='free' AND chapter.price_amount=0)
      OR (
          chapter.access_level='paid' AND chapter.price_amount>0
          AND EXISTS (
              SELECT 1
              FROM chapter_encrypted_release_events AS encrypted
              JOIN chapter_versions AS current_version
                ON current_version.id=encrypted.version_id
               AND current_version.chapter_id=encrypted.chapter_id
              JOIN storage_objects AS object
                ON object.id=encrypted.storage_object_id
               AND object.id=current_version.storage_object_id
              WHERE encrypted.chapter_id=chapter.id
                AND encrypted.version_id=publication.version_id
                AND object.state='attached'
                AND object.object_kind='chapter'
          )
      )
  );

COMMENT ON VIEW reader_followed_release_candidates IS
    'Actor-scoped metadata of first visible free and verified paid releases after explicit follow; no entitlement.';
