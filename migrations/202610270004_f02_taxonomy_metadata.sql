-- B/F-02: canonical author taxonomy for existing and new books.
-- Old rows default to empty arrays, without changing visibility.
ALTER TABLE books
    ADD COLUMN tags TEXT[] NOT NULL DEFAULT '{}'::text[]
        CHECK (
            cardinality(tags) <= 8
            AND array_position(tags, NULL) IS NULL
            AND array_position(tags, '') IS NULL
        ),
    ADD COLUMN fandoms TEXT[] NOT NULL DEFAULT '{}'::text[]
        CHECK (
            cardinality(fandoms) <= 8
            AND array_position(fandoms, NULL) IS NULL
            AND array_position(fandoms, '') IS NULL
        );

-- Only public, non-blocked and non-deleted material is searchable.
CREATE INDEX books_public_discovery_tags_idx
    ON books USING GIN (tags)
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted;

CREATE INDEX books_public_discovery_fandoms_idx
    ON books USING GIN (fandoms)
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted;
