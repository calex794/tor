-- B: NEwBOO book-level likes. One active like per authenticated reader.
-- Unlike stars or chapter thanks, this conveys no rating, reading, or purchase right.
-- A removed like is not counted; concurrent duplicate actions are idempotent.
CREATE TABLE book_likes (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    liked_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, book_id)
);

-- Public detail and protected author book statistics count by book_id.
CREATE INDEX book_likes_book_count_idx ON book_likes (book_id);
