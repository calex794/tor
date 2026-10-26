-- C-08/S007: explicit personal bookmarks are not progress or purchases.
-- Only reader-owned rows; no chapter/version/entitlement columns are present.
CREATE TABLE reader_library_items (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY CHECK (id > 0),
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, book_id)
);

-- Keyset pagination newest-first without scanning other users' bookmarks.
CREATE INDEX reader_library_items_user_cursor_idx
    ON reader_library_items (user_id, id DESC);
