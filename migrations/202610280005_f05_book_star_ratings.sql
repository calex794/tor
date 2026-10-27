-- F-05 / S008: independent 1..5 opinions of the literary work and its translation.
-- Personal pairwise tiers are deliberately unrelated to these book reactions.
-- No cached aggregate totals: a user's deletion/erasure cannot leave stale votes.
-- Ratings are private per user, and only published books have a public summary.
CREATE TABLE user_book_star_ratings (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    rating_kind TEXT NOT NULL CHECK (rating_kind IN ('work','translation')),
    rating SMALLINT NOT NULL CHECK (rating BETWEEN 1 AND 5),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, book_id, rating_kind)
);

-- Index-only per-book summary from real votes; no mutable materialized sum.
-- Never use the pairwise ranking table as a public score.
CREATE INDEX user_book_star_ratings_book_kind
    ON user_book_star_ratings (book_id, rating_kind) INCLUDE (rating);
