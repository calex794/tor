-- E-05/S014: following a creator account is distinct from following a book.
-- The canonical active period is the only authority for future subscriber discounts.
-- Source is a B-validated public book owner, not arbitrary client attribution.
CREATE TABLE creator_follow_periods (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    follower_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    creator_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    source_book_id BIGINT REFERENCES books(id) ON DELETE SET NULL,
    source_kind TEXT NOT NULL DEFAULT 'book_page' CHECK (source_kind='book_page'),
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    ended_at TIMESTAMPTZ,
    CONSTRAINT creator_follows_not_self CHECK (follower_user_id<>creator_user_id),
    CONSTRAINT creator_follows_valid_period CHECK (
        ended_at IS NULL OR ended_at>=started_at
    )
);
-- Partial unique index serializes active state independent of the source Book.
CREATE UNIQUE INDEX creator_follow_one_active
    ON creator_follow_periods (follower_user_id, creator_user_id)
    WHERE ended_at IS NULL;
CREATE INDEX creator_follow_reader_active
    ON creator_follow_periods (follower_user_id, id DESC) WHERE ended_at IS NULL;
CREATE INDEX creator_follow_creator_active
    ON creator_follow_periods (creator_user_id, started_at)
    WHERE ended_at IS NULL;
CREATE INDEX creator_follow_source_book
    ON creator_follow_periods (source_book_id, started_at)
    WHERE source_book_id IS NOT NULL;
COMMENT ON TABLE creator_follow_periods IS
    'E-05. Unique contiguous creator follows, including immutable source of the first follow; not book_follow, paid Layer or checkout entitlement.';
