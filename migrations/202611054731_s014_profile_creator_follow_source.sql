-- S014 / E-05 / I: a public profile follow is a user-to-user action,
-- independent of discovering or owning a particular Book.
-- Preserve every historical book_page source (including ON DELETE SET NULL).
ALTER TABLE creator_follow_periods
    DROP CONSTRAINT creator_follow_periods_source_kind_check;

ALTER TABLE creator_follow_periods
    ADD CONSTRAINT creator_follow_periods_source_kind_check
    CHECK (
        source_kind = 'book_page'
        OR (source_kind = 'profile' AND source_book_id IS NULL)
    );

COMMENT ON COLUMN creator_follow_periods.source_kind IS
    'book_page: B-verified Book owner at first follow; profile: active target user, no source Book. Existing follow periods are immutable.';
