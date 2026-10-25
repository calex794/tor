-- BKR-89: stable UTF-16 text position within a published immutable reading block.
-- Previously stored paragraph bookmarks remain at the start of that block.
ALTER TABLE reader_book_positions
    ADD COLUMN char_offset INTEGER NOT NULL DEFAULT 0
    CHECK (char_offset BETWEEN 0 AND 100000);
