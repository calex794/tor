-- BKR-87: books have one accountable owner; legacy/catalog fixtures remain unowned.
ALTER TABLE books ADD COLUMN owner_user_id BIGINT REFERENCES users(id) ON DELETE RESTRICT;
CREATE INDEX books_owner_idx ON books(owner_user_id, id)
    WHERE owner_user_id IS NOT NULL;

ALTER TABLE chapters
    ADD COLUMN edit_revision BIGINT NOT NULL DEFAULT 0 CHECK (edit_revision >= 0),
    ADD COLUMN price_amount BIGINT NOT NULL DEFAULT 0 CHECK (price_amount >= 0),
    ADD CONSTRAINT chapters_price_matches_access CHECK (
        (access_level = 'free' AND price_amount = 0)
        OR (access_level = 'paid' AND price_amount > 0)
    ) NOT VALID;

-- Existing imported paid chapters may not have prices. The NOT VALID constraint
-- protects every future write without inventing a price for old records.
-- New chapter content can be reused by hash without mutating prior versions.
-- Existing imported versions may have NULL until a separate reconciliation.
ALTER TABLE chapter_versions
    ADD COLUMN content_sha256 BYTEA CHECK (
        content_sha256 IS NULL OR octet_length(content_sha256) = 32
    );
CREATE UNIQUE INDEX chapter_versions_same_content
    ON chapter_versions(chapter_id, content_sha256)
    WHERE content_sha256 IS NOT NULL;

-- Claims and results live in the SAME transaction as the business mutation.
-- A collision waits for the winning transaction to commit, then replays or 409s.
CREATE TABLE book_create_commands (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    command_key TEXT NOT NULL CHECK (char_length(command_key) = 43),
    payload_hash BYTEA NOT NULL CHECK (octet_length(payload_hash) = 32),
    book_id BIGINT REFERENCES books(id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY(user_id, command_key)
);

CREATE TABLE chapter_save_commands (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    command_key TEXT NOT NULL CHECK (char_length(command_key) = 43),
    payload_hash BYTEA NOT NULL CHECK (octet_length(payload_hash) = 32),
    chapter_id BIGINT REFERENCES chapters(id) ON DELETE RESTRICT,
    version_id BIGINT REFERENCES chapter_versions(id) ON DELETE RESTRICT,
    saved_revision BIGINT CHECK (saved_revision >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY(user_id, command_key)
);

-- A durable first-release fact, NOT a delivered notification.
CREATE TABLE chapter_first_release_events (
    chapter_id BIGINT PRIMARY KEY REFERENCES chapters(id) ON DELETE RESTRICT,
    version_id BIGINT NOT NULL REFERENCES chapter_versions(id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);