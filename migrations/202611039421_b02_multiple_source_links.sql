-- S019: one actual original may have multiple known external links.
-- A Rulate page is an optional mirror/translation link, not automatically
-- the Chinese original. Users may supply URLs from any valid public host.
-- Claims are language-specific. Historical book rows are never overwritten.
CREATE TABLE book_registration_links (
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    target_language TEXT NOT NULL CHECK (char_length(target_language) BETWEEN 1 AND 32),
    link_identity TEXT NOT NULL CHECK (char_length(link_identity) BETWEEN 1 AND 256),
    source_url TEXT NOT NULL CHECK (char_length(source_url) BETWEEN 1 AND 512),
    PRIMARY KEY (book_id, link_identity),
    UNIQUE (target_language, link_identity)
);

-- Legacy primary links had no entry in book_registration_links.
-- Exact historical URLs remain findable without a full table scan.
CREATE INDEX books_registration_existing_urls_idx
    ON books(target_language, source_url)
    WHERE NOT is_deleted AND source_url <> '';
