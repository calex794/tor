-- C/F F-05: a verified purchased pinned version can produce a C completion.
-- Free completions are unchanged (purchase_id NULL); never mutate applied
-- migrations or treat a progress marker as entitlement.
-- The paid purchase is always checked by D's canonical function in the
-- same transaction as the C cursor and durable outbox commit.
ALTER TABLE reader_chapter_completions
    ADD COLUMN purchase_id BIGINT
        REFERENCES chapter_purchase_facts(id) ON DELETE CASCADE;

-- First completion per user/book/chapter is already enforced by the PK.
-- One purchase can only be associated with one immutable purchase fact.
CREATE INDEX reader_chapter_completions_paid_purchase
    ON reader_chapter_completions (purchase_id)
    WHERE purchase_id IS NOT NULL;
