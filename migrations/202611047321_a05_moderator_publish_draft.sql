-- MOD-C03: NEwBOO publish/draft are additional immutable moderation decisions.
-- The original hold/hidden/blocked audit is preserved, not rewritten.
ALTER TABLE moderation_chapter_decision_events
    DROP CONSTRAINT moderation_chapter_decision_before_allowed,
    DROP CONSTRAINT moderation_chapter_decision_after_allowed,
    ADD CONSTRAINT moderation_chapter_decision_before_allowed
       CHECK (before_state IN ('none','hold','hidden','blocked','draft','published')),
    ADD CONSTRAINT moderation_chapter_decision_after_allowed
       CHECK (after_state IN ('none','hold','hidden','blocked','draft','published'));
