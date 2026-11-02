-- S019 / B-02: deterministic Rulate work identity across known book URL variants.
-- No backfill or uniqueness constraint on historical books: duplicates may already exist.
-- The checked identity is a public work number, never a private user identifier.
CREATE FUNCTION book_registration_rulate_work_id(raw_url TEXT)
RETURNS TEXT
LANGUAGE SQL IMMUTABLE STRICT PARALLEL SAFE
AS $rulate$
    SELECT CASE
        WHEN lower(raw_url) ~ '^https?://(www\.)?(tl\.)?rulate\.ru(:80|:443)?/book/[0-9]{1,18}/?([?][^#[:space:]]*)?$'
        THEN NULLIF(
            ltrim(substring(lower(raw_url) FROM '/book/([0-9]{1,18})'), '0'),
            ''
        )
        ELSE NULL
    END
$rulate$;

CREATE INDEX books_registration_rulate_work_idx
    ON books(target_language, book_registration_rulate_work_id(source_url))
    WHERE NOT is_deleted AND source_url <> '';

-- Historical records can have original/alternate titles without a matching
-- book_registration_titles claim. Preserve those records and check their
-- normalized aliases through an indexed, bounded (at most four) array.
CREATE FUNCTION book_registration_legacy_alias_keys(
    old_original TEXT, old_alts TEXT[]
)
RETURNS TEXT[]
LANGUAGE SQL IMMUTABLE STRICT PARALLEL SAFE
AS $legacy_aliases$
    SELECT array_remove(
        array_remove(
            ARRAY[
                book_registration_legacy_title_key(old_original),
                book_registration_legacy_title_key(old_alts[1]),
                book_registration_legacy_title_key(old_alts[2]),
                book_registration_legacy_title_key(old_alts[3])
            ]::text[],
            NULL
        ),
        ''
    )
$legacy_aliases$;

CREATE INDEX books_registration_legacy_aliases_idx
    ON books USING gin (
        book_registration_legacy_alias_keys(original_title, alt_titles)
    )
    WHERE NOT is_deleted
      AND (original_title <> '' OR cardinality(alt_titles) > 0);
