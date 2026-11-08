-- E-05/S014: attribute a first release to the verified account owner.
-- Existing rows remain NULL: historical book ownership cannot be reconstructed safely.
ALTER TABLE chapter_first_release_events
    ADD COLUMN creator_user_id BIGINT REFERENCES users(id) ON DELETE RESTRICT;

CREATE INDEX chapter_first_release_creator_feed
    ON chapter_first_release_events (creator_user_id, created_at, chapter_id)
    WHERE creator_user_id IS NOT NULL;

-- Include closed intervals too: releases made while a subscription was active
-- remain in the reader's on-site feed after unsubscribe. New periods do not
-- retroactively include releases from the gap between periods.
CREATE INDEX creator_follow_release_feed
    ON creator_follow_periods (creator_user_id, started_at, follower_user_id)
    INCLUDE (ended_at);

-- Book-follow and creator-follow branches have the same columns. UNION (not
-- UNION ALL) deduplicates one chapter when the reader follows both relations.
CREATE OR REPLACE VIEW reader_followed_release_candidates AS
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
  )
UNION
SELECT follow.follower_user_id AS user_id,
       book.id AS book_id,
       book.title AS book_title,
       chapter.id AS chapter_id,
       chapter.title AS chapter_title,
       chapter.chapter_no AS chapter_number,
       chapter.access_level,
       chapter.price_amount AS price_minor,
       release.created_at AS released_at,
       (EXTRACT(EPOCH FROM release.created_at) * 1000000)::bigint AS released_micros
FROM creator_follow_periods AS follow
JOIN chapter_first_release_events AS release
  ON release.creator_user_id=follow.creator_user_id
 AND release.created_at>=follow.started_at
 AND (follow.ended_at IS NULL OR release.created_at<follow.ended_at)
JOIN chapters AS chapter ON chapter.id=release.chapter_id
JOIN books AS book ON book.id=chapter.book_id
JOIN chapter_publications AS publication ON publication.chapter_id=chapter.id
WHERE book.visibility='public'
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

COMMENT ON COLUMN chapter_first_release_events.creator_user_id IS
    'Verified book owner at first publication; NULL for historical events predating E-05 snapshot.';
COMMENT ON VIEW reader_followed_release_candidates IS
    'Actor-scoped first-release metadata after explicit book or creator follow; deduplicated; no entitlement or external delivery.';
