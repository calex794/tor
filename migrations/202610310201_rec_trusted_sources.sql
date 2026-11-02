-- C-owned verified read-event port. An outbox payload alone is not evidence.
ALTER TABLE reader_chapter_starts ADD COLUMN purchase_id bigint REFERENCES chapter_purchase_facts(id) ON DELETE CASCADE;
CREATE VIEW rec_verified_read_events WITH (security_barrier=true) AS
SELECT e.id AS event_id,c.user_id,c.book_id,c.chapter_id,
       c.completed_at AS occurred_at,'chapter_finished'::text AS kind,
       NOT EXISTS(SELECT 1 FROM reader_chapter_completions old
           WHERE old.user_id=c.user_id AND old.book_id=c.book_id
             AND (old.completed_at,old.source_event_id)<(c.completed_at,c.source_event_id)) AS first_for_book
FROM reader_chapter_completions c
JOIN outbox_events e ON e.id=c.source_event_id
JOIN chapters ch ON ch.id=c.chapter_id AND ch.book_id=c.book_id
WHERE e.event_type='ChapterCompletedV1' AND e.schema_version=1
  AND e.aggregate_kind='chapter' AND e.aggregate_id=c.chapter_id
  AND e.payload->>'user_id'=c.user_id::text AND e.payload->>'book_id'=c.book_id::text
  AND e.payload->>'chapter_id'=c.chapter_id::text AND e.payload->>'version_id'=c.first_version_id::text
  AND ((c.purchase_id IS NULL AND e.payload->>'authority'='C-verified-free-position' AND NOT(e.payload ? 'purchase_id'))
       OR (c.purchase_id IS NOT NULL AND e.payload->>'authority'='C-verified-purchased-position'
           AND e.payload->>'purchase_id'=c.purchase_id::text
           AND tor_purchase_version_readable(c.user_id,c.purchase_id,c.chapter_id,c.first_version_id)))
UNION ALL
SELECT e.id,c.user_id,c.book_id,c.chapter_id,c.started_at,'chapter_started'::text,
       NOT EXISTS(SELECT 1 FROM reader_chapter_starts old
           WHERE old.user_id=c.user_id AND old.book_id=c.book_id
             AND (old.started_at,old.source_event_id)<(c.started_at,c.source_event_id))
FROM reader_chapter_starts c
JOIN outbox_events e ON e.id=c.source_event_id
JOIN chapters ch ON ch.id=c.chapter_id AND ch.book_id=c.book_id
WHERE e.event_type='ChapterStartedV1' AND e.schema_version=1
  AND e.aggregate_kind='chapter' AND e.aggregate_id=c.chapter_id
  AND e.payload->>'user_id'=c.user_id::text AND e.payload->>'book_id'=c.book_id::text
  AND e.payload->>'chapter_id'=c.chapter_id::text AND e.payload->>'version_id'=c.first_version_id::text
  AND ((c.purchase_id IS NULL AND e.payload->>'authority'='C-verified-free-start' AND NOT(e.payload ? 'purchase_id'))
       OR (c.purchase_id IS NOT NULL AND e.payload->>'authority'='C-verified-purchased-start'
           AND e.payload->>'purchase_id'=c.purchase_id::text
           AND tor_purchase_version_readable(c.user_id,c.purchase_id,c.chapter_id,c.first_version_id)));
