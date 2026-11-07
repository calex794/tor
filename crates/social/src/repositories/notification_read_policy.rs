//! S015: re-check release visibility when reading or acknowledging the bell.
//! Notification ownership is necessary, but a stored delivery is not an access grant.
//! The outer query uses alias `n`; all SQL here is static, never user input.
pub(super) const VISIBLE: &str = r#"
(n.kind <> 'chapter_created' OR EXISTS (
    SELECT 1 FROM outbox_events e
    JOIN chapters c ON c.id=e.aggregate_id
    JOIN books b ON b.id=c.book_id
    JOIN users recipient ON recipient.id=n.recipient_user_id
      AND recipient.account_state='active'
    JOIN chapter_publications publication ON publication.chapter_id=c.id
    WHERE e.id=n.source_event_id AND e.event_type='reader.chapter.released'
      AND e.payload->>'recipient_user_id'=n.recipient_user_id::text
      AND e.payload->>'actor_user_id'=n.actor_user_id::text
      AND e.payload->>'object_id'=c.id::text
      AND e.payload->>'book_id'=b.id::text
      AND b.visibility='public' AND b.visibility_status='active'
      AND b.lifecycle_status<>'pending_deletion'
      AND NOT b.admin_hold AND NOT b.is_deleted
      AND (
          b.access_level='basic'
          OR (b.access_level='adult' AND recipient.adult_enabled)
          OR (b.access_level='pro_adult' AND EXISTS (
              SELECT 1 FROM user_level_grants age_grant
              WHERE age_grant.user_id=recipient.id
                AND age_grant.level='pro_adult' AND age_grant.is_active
          ))
      )
      AND NOT EXISTS (
          SELECT 1 FROM social_user_blocks block
          WHERE (block.blocker_id=n.actor_user_id
                 AND block.blocked_id=n.recipient_user_id)
             OR (block.blocker_id=n.recipient_user_id
                 AND block.blocked_id=n.actor_user_id)
      )
      AND c.visibility='public' AND NOT c.admin_hold AND NOT c.is_deleted
      AND (c.reader_released_at IS NULL
           OR c.reader_released_at<=clock_timestamp())
      AND (
          (c.access_level='free' AND c.price_amount=0)
          OR (c.access_level='paid' AND c.price_amount>0 AND EXISTS (
              SELECT 1 FROM chapter_encrypted_release_events encrypted
              JOIN chapter_versions version ON version.id=encrypted.version_id
                AND version.chapter_id=c.id
              JOIN storage_objects object ON object.id=encrypted.storage_object_id
                AND object.id=version.storage_object_id
              WHERE encrypted.chapter_id=c.id
                AND encrypted.version_id=publication.version_id
                AND object.state='attached' AND object.object_kind='chapter'
                AND object.verified_at IS NOT NULL
          ))
      )
))"#;
