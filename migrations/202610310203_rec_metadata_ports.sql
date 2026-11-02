-- B-owned metadata mapping for successful reader SSR surfaces.
CREATE VIEW rec_public_chapter_links WITH (security_barrier=true) AS
SELECT c.id AS chapter_id,c.book_id
FROM chapters c JOIN rec_public_books b ON b.id=c.book_id
JOIN chapter_publications p ON p.chapter_id=c.id
WHERE c.visibility='public' AND NOT c.admin_hold AND NOT c.is_deleted
  AND (c.reader_released_at IS NULL OR c.reader_released_at<=now());

-- C first-start eligibility must not turn a historically completed book into
-- a newly acquired reader when REC-13 is installed later.
CREATE VIEW rec_verified_read_events_v2 WITH (security_barrier=true) AS
SELECT v.event_id,v.user_id,v.book_id,v.chapter_id,v.occurred_at,v.kind,
  v.first_for_book AND (v.kind<>'chapter_started' OR NOT EXISTS(
    SELECT 1 FROM reader_chapter_completions c
    WHERE c.user_id=v.user_id AND c.book_id=v.book_id AND c.completed_at<=v.occurred_at
  )) AS first_for_book
FROM rec_verified_read_events v;
