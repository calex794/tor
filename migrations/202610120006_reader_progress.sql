-- BKR-88: private progress is NOT a library purchase or a reading license.
-- Each user has one current continuation point per book.
CREATE TABLE reader_book_positions (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    chapter_id BIGINT NOT NULL REFERENCES chapters(id) ON DELETE CASCADE,
    chapter_no INTEGER NOT NULL CHECK (chapter_no > 0),
    chapter_revision BIGINT NOT NULL CHECK (chapter_revision >= 0),
    version_id BIGINT NOT NULL,
    block_index INTEGER NOT NULL CHECK (block_index BETWEEN 1 AND 200),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, book_id),
    FOREIGN KEY (chapter_id, version_id)
        REFERENCES chapter_versions(chapter_id, id)
);
CREATE INDEX reader_book_positions_recent_idx
    ON reader_book_positions (user_id, updated_at DESC, book_id DESC);