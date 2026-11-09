-- S049: explicitly requested administrator hide of a single free-edition comment.
-- Original review decision evidence remains immutable; no automatic sanctions.
-- No changes to a chapter edition, publication, author access, grants or finance.
ALTER TABLE social_free_comment_review_events
    ADD COLUMN moderation_action TEXT NOT NULL DEFAULT 'none'
        CHECK (
            moderation_action = 'none'
            OR (moderation_action = 'hide_free_comment' AND decision = 'reviewed')
        );
