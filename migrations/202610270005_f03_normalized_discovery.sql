-- F-03/S001: NFKC normalization and Cyrillic ё/е folding for public title/author search.
-- The original NEwBOO title normalization uses NFKC and maps ё to е.
-- Query and indexed document use exactly the same PostgreSQL expression.
-- Existing visibility predicates remain in the partial index.
CREATE INDEX books_public_discovery_terms_nfkc_idx
    ON books USING GIN ((
        to_tsvector(
            'simple'::regconfig,
            translate(lower(normalize(title, NFKC)), 'ё', 'е')
        )
        ||
        to_tsvector(
            'simple'::regconfig,
            translate(lower(normalize(author_name, NFKC)), 'ё', 'е')
        )
    ))
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted;

-- The index for the old unnormalized FTS expression is now redundant.
DROP INDEX books_public_discovery_terms_idx;
