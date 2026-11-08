-- S028 / B: an opt-in Team-to-Book reference, never a Book ownership transfer.
-- This migration only lands in source / isolated CI. Do not roll out to production.
CREATE TABLE translation_team_books (
    team_id BIGINT NOT NULL REFERENCES translation_teams(id) ON DELETE RESTRICT,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    added_by_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (team_id, book_id)
);
CREATE INDEX translation_team_books_active_team
    ON translation_team_books(team_id, created_at DESC)
    WHERE is_active;
