-- B-owned public availability metadata only; not a paid access grant.
CREATE VIEW rec_free_readable_books WITH (security_barrier=true) AS
SELECT DISTINCT b.id AS book_id
FROM rec_public_books b JOIN chapters c ON c.book_id=b.id
JOIN chapter_publications p ON p.chapter_id=c.id
WHERE c.visibility='public' AND c.access_level='free'
  AND NOT c.admin_hold AND NOT c.is_deleted
  AND (c.reader_released_at IS NULL OR c.reader_released_at<=now());
