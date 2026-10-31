-- C-18: explicit per-user read/unread marker for each confirmed V2+ event.
-- Not a delivery receipt, purchase entitlement or proof of opening the chapter.
-- Separate from C-16's (user_id,chapter_id) V1 read markers.
ALTER TABLE notification_event_recipient_intents
    ADD COLUMN read_at TIMESTAMPTZ;

COMMENT ON COLUMN notification_event_recipient_intents.read_at IS
    'Reader explicit read-state per ChapterPublished event; NULL is unread.';

CREATE INDEX notification_recipient_user_page_idx
    ON notification_event_recipient_intents(user_id,staged_at DESC,event_id DESC);

-- One canonical, re-evaluated eligibility predicate for both listing and
-- read-state writes. Visibility, follow and current published V2 are checked
-- on every request; a stale/held/revoked event is not actionable.
CREATE VIEW reader_staged_revision_candidates AS
SELECT intent.user_id,intent.event_id,intent.staged_at,intent.read_at,
       book.id AS book_id,book.title AS book_title,
       chapter.id AS chapter_id,chapter.title AS chapter_title,
       chapter.chapter_no,version.version_no,
       (chapter.access_level='paid') AS is_paid
FROM notification_event_recipient_intents AS intent
               JOIN notification_event_fanout_scans AS scan
                 ON scan.event_id=intent.event_id
                AND scan.completed_at IS NOT NULL
               JOIN event_deliveries AS receipt
                 ON receipt.event_id=intent.event_id
                AND receipt.consumer='subscription-inbox-v1'
               JOIN outbox_events AS event ON event.id=intent.event_id
               JOIN chapters AS chapter ON chapter.id=event.aggregate_id
                 AND event.payload->'chapter_id'=to_jsonb(chapter.id)
               JOIN books AS book ON book.id=chapter.book_id
                 AND event.payload->'book_id'=to_jsonb(book.id)
               JOIN chapter_publications AS publication
                 ON publication.chapter_id=chapter.id
                AND event.payload->'version_id'=to_jsonb(publication.version_id)
               JOIN chapter_versions AS version ON version.id=publication.version_id
               JOIN reader_book_follows AS follows ON follows.book_id=book.id
                 AND follows.user_id=intent.user_id
               WHERE event.event_type='ChapterPublished'
                 AND event.schema_version=1
                 AND event.aggregate_kind='chapter'
                 AND event.payload->'first_release'='false'::jsonb
                 AND follows.created_at<=event.created_at
                 AND book.visibility='public'
                 AND NOT book.admin_hold AND NOT book.is_deleted
                 AND chapter.visibility='public'
                 AND NOT chapter.admin_hold AND NOT chapter.is_deleted
                 AND (chapter.reader_released_at IS NULL
                      OR chapter.reader_released_at<=now())
                 AND (
                   (chapter.access_level='free' AND chapter.price_amount=0)
                   OR (
                     chapter.access_level='paid' AND chapter.price_amount>0
                     AND EXISTS (
                       SELECT 1 FROM chapter_encrypted_release_events AS encrypted
                       JOIN chapter_versions AS protected
                         ON protected.id=encrypted.version_id
                        AND protected.chapter_id=encrypted.chapter_id
                       JOIN storage_objects AS object
                         ON object.id=encrypted.storage_object_id
                        AND object.id=protected.storage_object_id
                       WHERE encrypted.chapter_id=chapter.id
                         AND encrypted.version_id=publication.version_id
                         AND object.state='attached'
                         AND object.object_kind='chapter'
                     )
                   )
                 )

                 AND version.version_no>1;

COMMENT ON VIEW reader_staged_revision_candidates IS
    'Live eligible per-reader staged V2+ notification metadata; no paid body, no delivery or purchase grant.';
