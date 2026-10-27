-- F-05/REC-08: private personal comparison ranking, NOT public popularity.
-- Durable per-user fencing head. The application only performs DML.
CREATE TABLE user_book_ranking_head (
    user_id BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    revision BIGINT NOT NULL DEFAULT 0 CHECK (revision >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Sparse tiers preserve tied books and avoid rewriting all ranks on each vote.
CREATE TABLE user_book_rank (
    user_id BIGINT NOT NULL REFERENCES user_book_ranking_head(user_id) ON DELETE CASCADE,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    tier BIGINT NOT NULL,
    comparisons_count BIGINT NOT NULL DEFAULT 0 CHECK (comparisons_count >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, book_id)
);
CREATE INDEX user_book_rank_by_tier
    ON user_book_rank (user_id, tier, book_id);

-- A user has at most one live comparison placement.
CREATE TABLE user_book_ranking_state (
    user_id BIGINT PRIMARY KEY REFERENCES user_book_ranking_head(user_id) ON DELETE CASCADE,
    placing_book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    low_position INTEGER NOT NULL CHECK (low_position >= 0),
    high_position INTEGER NOT NULL CHECK (high_position >= low_position),
    state_version BIGINT NOT NULL CHECK (state_version > 0),
    placement_epoch BIGINT NOT NULL CHECK (placement_epoch > 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Append-only answer evidence, scoped to the placement epoch.
-- This log doubles as a durable skip record within one state version.
CREATE TABLE user_book_comparison (
    user_id BIGINT NOT NULL REFERENCES user_book_ranking_head(user_id) ON DELETE CASCADE,
    placement_epoch BIGINT NOT NULL CHECK (placement_epoch > 0),
    state_version BIGINT NOT NULL CHECK (state_version > 0),
    comparison_id TEXT NOT NULL CHECK (length(comparison_id) BETWEEN 20 AND 100),
    book_a_id BIGINT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    book_b_id BIGINT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    outcome TEXT NOT NULL
        CHECK (outcome IN ('a_wins','b_wins','tie','skip','defer_a','defer_b')),
    question_kind TEXT NOT NULL DEFAULT 'placement' CHECK (question_kind = 'placement'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, placement_epoch, comparison_id),
    CHECK (book_a_id <> book_b_id)
);
CREATE INDEX user_book_comparison_active_skips
    ON user_book_comparison (user_id, placement_epoch, state_version)
    WHERE outcome = 'skip';

-- Only a fresh trusted completion may release a defer; simple GET never does.
CREATE TABLE user_book_ranking_defer (
    user_id BIGINT NOT NULL REFERENCES user_book_ranking_head(user_id) ON DELETE CASCADE,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    deferred_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, book_id)
);
