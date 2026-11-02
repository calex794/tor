-- S019 / B-02: advisory matching of PUBLIC registered titles, including
-- original and alternative names in any language. Private drafts are never
-- indexed into the public suggestion index or returned by the endpoint.
-- This is additive; do not rewrite migration versions already applied.
CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE FUNCTION book_registration_public_title_text(
    book_title TEXT, original_title TEXT, alternative_titles TEXT[]
)
RETURNS TEXT
LANGUAGE SQL IMMUTABLE PARALLEL SAFE
AS $suggest$
    SELECT replace(
        lower(
            coalesce(book_title, '') || ' ' ||
            coalesce(original_title, '') || ' ' ||
            coalesce(array_to_string(alternative_titles, ' '), '')
        ),
        'ё', 'е'
    )
$suggest$;

CREATE INDEX books_public_registration_title_trgm_idx
    ON books USING GIN (
        book_registration_public_title_text(title, original_title, alt_titles)
        gin_trgm_ops
    )
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted;
