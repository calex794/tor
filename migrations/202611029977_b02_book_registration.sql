-- B-02/S019: persist registration metadata without changing published visibility.
-- Existing books retain their historical values and do not acquire title claims.
ALTER TABLE books
    ADD COLUMN original_title TEXT NOT NULL DEFAULT ''
        CHECK (char_length(original_title) <= 200),
    ADD COLUMN original_language TEXT NOT NULL DEFAULT ''
        CHECK (char_length(original_language) <= 32),
    ADD COLUMN source_url TEXT NOT NULL DEFAULT ''
        CHECK (char_length(source_url) <= 512),
    ADD COLUMN release_year INTEGER
        CHECK (release_year BETWEEN 1000 AND 9999),
    ADD COLUMN project_status TEXT NOT NULL DEFAULT ''
        CHECK (project_status IN ('', 'ongoing', 'completed', 'paused', 'planned')),
    ADD COLUMN catalog_section TEXT NOT NULL DEFAULT ''
        CHECK (char_length(catalog_section) <= 80),
    ADD COLUMN alt_titles TEXT[] NOT NULL DEFAULT '{}'::text[]
        CHECK (
            cardinality(alt_titles) <= 3
            AND array_position(alt_titles, NULL) IS NULL
            AND array_position(alt_titles, '') IS NULL
        );

-- New registrations atomically claim all normalized title aliases.
-- This avoids a risky backfill/unique constraint over historical duplicates.
-- UNIQUE(source_identity, target_language) on books still owns source identity.
CREATE TABLE book_registration_titles (
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    target_language TEXT NOT NULL,
    normalized_title TEXT NOT NULL
        CHECK (char_length(normalized_title) BETWEEN 1 AND 200),
    PRIMARY KEY (book_id, normalized_title),
    UNIQUE (target_language, normalized_title)
);

CREATE INDEX book_registration_titles_book_idx
    ON book_registration_titles (book_id);
