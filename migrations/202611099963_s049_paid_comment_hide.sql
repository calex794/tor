-- S049: explicit administrator-only action on the exact reported paid V1 comment.
-- The existing immutable review trigger also protects this column.
-- No D purchase, edition body, S3 object, or finance state is modified.
ALTER TABLE social_paid_comment_review_events
    ADD COLUMN moderation_action TEXT NOT NULL DEFAULT 'none'
        CHECK (moderation_action IN ('none','hide_paid_comment'));
