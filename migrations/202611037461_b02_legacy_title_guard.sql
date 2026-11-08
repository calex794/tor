-- B-02 / S019: guard new registrations against titles of preexisting books.
-- Historical titles can legitimately be duplicated. Do not rewrite or force
-- uniqueness on those rows, and never expose private rows through a lookup.
-- The key matches the common Rust title normalization: lowercase, ё -> е,
-- collapse punctuation and spaces, strip leading/trailing separators.
CREATE FUNCTION book_registration_legacy_title_key(raw_title TEXT)
RETURNS TEXT
LANGUAGE SQL IMMUTABLE STRICT PARALLEL SAFE
AS $registration$
    SELECT btrim(
        regexp_replace(
            replace(lower(raw_title), 'ё', 'е'),
            '[^[:alnum:]]+', ' ', 'g'
        )
    )
$registration$;

-- Keep checks indexed even with a six-figure catalog; new registration
-- reads do not scan the historical table. The index is maintained when
-- a legacy book is renamed, and deleted books are intentionally excluded.
CREATE INDEX books_registration_legacy_title_idx
    ON books(target_language, book_registration_legacy_title_key(title))
    WHERE NOT is_deleted;
