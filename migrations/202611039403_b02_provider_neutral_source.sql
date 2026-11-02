-- S019 / B-02: one URL-identity rule for any permitted HTTP(S) source.
-- Historical books keep their source_identity and visibility unchanged.
-- The old provider-specific Rulate rule was an incorrect product assumption.

CREATE FUNCTION book_registration_normalized_source_url(raw_url TEXT)
RETURNS TEXT
LANGUAGE plpgsql IMMUTABLE STRICT PARALLEL SAFE
AS $source$
DECLARE
    clean_url TEXT := btrim(raw_url);
    parts TEXT[];
    canonical_host TEXT;
    canonical_path TEXT;
BEGIN
    IF length(clean_url) > 512
        OR clean_url ~ '[[:space:]]'
        OR strpos(clean_url, '#') > 0
        OR strpos(clean_url, chr(92)) > 0
    THEN
        RETURN NULL;
    END IF;

    -- Preserve path case and the complete query: they can select distinct works.
    parts := regexp_match(
        clean_url,
        '^https?://([a-z0-9.-]+)(:[0-9]+)?(/[^?#]*)?(\?[^#]*)?$',
        'i'
    );
    IF parts IS NULL OR parts[2] IS NOT NULL
        AND parts[2] NOT IN (':80', ':443')
    THEN
        RETURN NULL;
    END IF;

    canonical_host := rtrim(lower(parts[1]), '.');
    IF canonical_host NOT LIKE '%.%'
        OR canonical_host LIKE '%.local'
        OR canonical_host ~ '^[0-9.]+$'
    THEN
        RETURN NULL;
    END IF;
    IF left(canonical_host, 4) = 'www.' THEN
        canonical_host := substr(canonical_host, 5);
    END IF;

    canonical_path := rtrim(coalesce(parts[3], '/'), '/');
    IF canonical_path = '' THEN
        canonical_path := '/';
    END IF;
    RETURN 'https://' || canonical_host || canonical_path || coalesce(parts[4], '');
END;
$source$;

CREATE INDEX books_registration_normalized_source_idx
    ON books(target_language, book_registration_normalized_source_url(source_url))
    WHERE NOT is_deleted AND source_url <> '';

-- Supersede only the obsolete provider-specific historical lookup.
DROP INDEX IF EXISTS books_registration_rulate_work_idx;
DROP FUNCTION IF EXISTS book_registration_rulate_work_id(TEXT);
