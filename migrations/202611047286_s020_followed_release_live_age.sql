-- S020/#486: first-release metadata remains private to currently eligible readers.
-- This view feeds on-site followed Book/creator updates and unread counts;
-- no purchase or read entitlement is created. A saved follow survives revocation.
-- No production rollout. SQLx migration on disposable CI databases only.
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
JOIN users AS recipient ON recipient.id=follow.user_id AND recipient.account_state='active'
JOIN books AS book ON book.id=follow.book_id
JOIN chapters AS chapter ON chapter.book_id=book.id
JOIN chapter_first_release_events AS release ON release.chapter_id=chapter.id
JOIN chapter_publications AS publication ON publication.chapter_id=chapter.id
WHERE release.created_at>=follow.created_at
  AND book.visibility='public' AND book.visibility_status='active'
  AND book.lifecycle_status<>'pending_deletion'
  AND NOT book.admin_hold AND NOT book.is_deleted
  AND (
       book.access_level='basic'
       OR (book.access_level='adult' AND recipient.adult_enabled)
       OR (
           book.access_level='pro_adult'
           AND EXISTS (
               SELECT 1 FROM user_level_grants AS age_grant
               WHERE age_grant.user_id=recipient.id
                 AND age_grant.level='pro_adult' AND age_grant.is_active
           )
       )
  )
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
JOIN users AS recipient ON recipient.id=follow.follower_user_id AND recipient.account_state='active'
JOIN chapter_first_release_events AS release
  ON release.creator_user_id=follow.creator_user_id
 AND release.created_at>=follow.started_at
 AND (follow.ended_at IS NULL OR release.created_at<follow.ended_at)
JOIN chapters AS chapter ON chapter.id=release.chapter_id
JOIN books AS book ON book.id=chapter.book_id
JOIN chapter_publications AS publication ON publication.chapter_id=chapter.id
WHERE book.visibility='public' AND book.visibility_status='active'
  AND book.lifecycle_status<>'pending_deletion'
  AND NOT book.admin_hold AND NOT book.is_deleted
  AND (
       book.access_level='basic'
       OR (book.access_level='adult' AND recipient.adult_enabled)
       OR (
           book.access_level='pro_adult'
           AND EXISTS (
               SELECT 1 FROM user_level_grants AS age_grant
               WHERE age_grant.user_id=recipient.id
                 AND age_grant.level='pro_adult' AND age_grant.is_active
           )
       )
  )
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
    'Verified after-follow release metadata, gated by recipient live account, Book age, moderation and lifecycle. Paid metadata is not a read entitlement.';
