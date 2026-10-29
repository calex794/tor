-- C-15 / Loop 1: explicit follow of a book, independent of library and purchases.
-- Following never grants access to a chapter or changes reader progress.
CREATE TABLE reader_book_follows (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, book_id)
);

-- Reader's followed books, most recent first; no cross-reader lookup.
CREATE INDEX reader_book_follows_user_recent_idx
    ON reader_book_follows (user_id, created_at DESC, book_id DESC);

-- Book-level aggregates for authors and later notification fan-out.
CREATE INDEX reader_book_follows_book_idx
    ON reader_book_follows (book_id, user_id);

COMMENT ON TABLE reader_book_follows IS
    'Explicit reader book-follow intent only; not a library item, purchase, or continuation-demand vote.';
