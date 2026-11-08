-- H-09/S023: one confirmed all-or-nothing chapter import per idempotency key.
-- A replay checks the full payload hash; changing the body with the same key is a conflict.
-- No chapter is published by the importer; the book must still be a private draft.
CREATE TABLE chapter_import_batches (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    command_key TEXT NOT NULL
        CHECK (char_length(command_key)=43),
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    content_sha256 BYTEA NOT NULL
        CHECK (octet_length(content_sha256)=32),
    chapter_count INTEGER NOT NULL
        CHECK (chapter_count BETWEEN 1 AND 500),
    committed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, command_key)
);
CREATE INDEX chapter_import_batches_book_idx
    ON chapter_import_batches (book_id, committed_at DESC);

COMMENT ON TABLE chapter_import_batches IS
    'Verified, owner-only batch command committed in the same PG transaction as all draft chapter versions.';
