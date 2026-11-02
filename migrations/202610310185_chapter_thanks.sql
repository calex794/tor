-- C/E: NEwBOO one-time chapter "thank you" by authenticated reader.
-- The original YDB key is (user_id, chapter_id). No personal events/IP logs.
-- Historical thanks are retained until their user/chapter/book is deleted.
CREATE TABLE chapter_thanks (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    chapter_id BIGINT NOT NULL REFERENCES chapters(id) ON DELETE CASCADE,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    first_thanked_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, chapter_id)
);

CREATE INDEX chapter_thanks_book_chapter_idx
    ON chapter_thanks (book_id, chapter_id);

-- Fast chapter-level counts; the (book_id, chapter_id) index cannot cover these.
CREATE INDEX chapter_thanks_chapter_user_idx
    ON chapter_thanks (chapter_id, user_id);

COMMENT ON TABLE chapter_thanks IS
    'One confirmed thank-you per registered user and accessible published chapter. Repeats do not increment.';
