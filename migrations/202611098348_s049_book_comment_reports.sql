-- S049: complaint on one visible public Book discussion comment.
-- Keep one pending Book-level report, but permit independent comment targets.
-- No chapter-edition bodies or access grants are stored here.
ALTER TABLE social_book_content_reports
  ADD COLUMN comment_id BIGINT REFERENCES discussion_comments(id) ON DELETE RESTRICT;

DROP INDEX social_book_content_reports_one_pending;
CREATE UNIQUE INDEX social_book_content_reports_one_pending
  ON social_book_content_reports(reporter_user_id,book_id)
  WHERE state='pending' AND comment_id IS NULL;

CREATE UNIQUE INDEX social_book_content_reports_one_comment_pending
  ON social_book_content_reports(reporter_user_id,comment_id)
  WHERE state='pending' AND comment_id IS NOT NULL;

CREATE INDEX social_book_content_reports_comment_lookup
  ON social_book_content_reports(comment_id)
  WHERE comment_id IS NOT NULL;

-- A comment ID must belong to the same Book and must not be an edition anchor.
-- Visibility is checked transactionally on submission and may later change.
CREATE FUNCTION s049_comment_report_target_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.comment_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM discussion_comments c WHERE c.id=NEW.comment_id
          AND c.book_id=NEW.book_id AND c.chapter_id IS NULL
          AND c.version_id IS NULL
    ) THEN
        RAISE EXCEPTION 'S049 comment does not belong to Book'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER s049_comment_report_target_guard
  BEFORE INSERT OR UPDATE OF comment_id,book_id ON social_book_content_reports
  FOR EACH ROW EXECUTE FUNCTION s049_comment_report_target_guard();
