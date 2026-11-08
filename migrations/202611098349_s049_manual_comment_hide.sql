-- S049: explicit administrator action for a confirmed public Book comment report.
-- Existing review rows keep action='none'. The preexisting immutable trigger
-- protects this action column as well; no auto-hold or chapter moderation.
ALTER TABLE social_book_content_review_events
  ADD COLUMN moderation_action TEXT NOT NULL DEFAULT 'none'
  CHECK (moderation_action IN ('none','hide_book_comment'));
