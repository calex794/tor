-- I/B/F F-03/S001: B writes canonical metadata, F reads only published rows.
-- Defaults preserve old fixtures and unchanged historical SQLx migrations.
ALTER TABLE books
    ADD COLUMN author_name TEXT NOT NULL DEFAULT ''
        CHECK (author_name = btrim(author_name) AND char_length(author_name) <= 160),
    ADD COLUMN genres TEXT[] NOT NULL DEFAULT '{}'::text[]
        CHECK (
            cardinality(genres) <= 8
            AND array_position(genres, NULL) IS NULL
            AND array_position(genres, '') IS NULL
        );

-- Tokenize title and author separately: malformed literal HTML in a title must
-- never swallow all following author lexemes.
CREATE INDEX books_public_discovery_terms_idx
    ON books USING GIN ((to_tsvector('simple'::regconfig, title) || to_tsvector('simple'::regconfig, author_name)))
    WHERE visibility = 'public' AND NOT admin_hold AND NOT is_deleted;

CREATE INDEX books_public_discovery_genres_idx
    ON books USING GIN (genres)
    WHERE visibility = 'public' AND NOT admin_hold AND NOT is_deleted;
