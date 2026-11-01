-- B/TAX-01. Persistent, administrator-owned canonical taxonomy; only public
-- facets seed discoverable suggestions (never harvest private draft metadata).
-- The existing TEXT[] columns remain the compatible F/REC read projection.
CREATE TABLE book_taxonomy_terms (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('genre','tag','fandom')),
    label TEXT NOT NULL CHECK (
        label = btrim(label) AND char_length(label) BETWEEN 1 AND 40
        AND label !~ '[[:cntrl:]]'
    ),
    search_key TEXT NOT NULL CHECK (char_length(search_key) BETWEEN 1 AND 80),
    status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active','merged','blocked')),
    merged_into_id BIGINT REFERENCES book_taxonomy_terms(id),
    UNIQUE(kind,id),
    UNIQUE(kind,search_key),
    CHECK (merged_into_id IS NULL OR merged_into_id <> id)
);
CREATE INDEX book_taxonomy_terms_prefix_idx
    ON book_taxonomy_terms(kind,search_key text_pattern_ops,id)
    WHERE status='active';

CREATE TABLE book_taxonomy_aliases (
    kind TEXT NOT NULL CHECK (kind IN ('genre','tag','fandom')),
    search_key TEXT NOT NULL CHECK (char_length(search_key) BETWEEN 1 AND 80),
    term_id BIGINT NOT NULL,
    PRIMARY KEY(kind,search_key),
    FOREIGN KEY(kind,term_id)
        REFERENCES book_taxonomy_terms(kind,id) ON DELETE CASCADE
);
CREATE INDEX book_taxonomy_aliases_prefix_idx
    ON book_taxonomy_aliases(kind,search_key text_pattern_ops,term_id);

CREATE TABLE book_taxonomy_links (
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('genre','tag','fandom')),
    term_id BIGINT NOT NULL,
    position SMALLINT NOT NULL CHECK (position BETWEEN 0 AND 7),
    PRIMARY KEY(book_id,kind,term_id),
    UNIQUE(book_id,kind,position),
    FOREIGN KEY(kind,term_id) REFERENCES book_taxonomy_terms(kind,id)
);
CREATE INDEX book_taxonomy_links_term_idx ON book_taxonomy_links(term_id,book_id);

-- Only visible, approved facet strings are eligible to be suggested.
-- NFKC is implemented by the existing B writer for new labels; migration
-- folds the known B labels and ё/е for search equivalence.
INSERT INTO book_taxonomy_terms(kind,label,search_key)
SELECT DISTINCT ON(kind,search_key) kind,name,search_key
FROM (
    SELECT kind,name,lower(replace(btrim(name),'ё','е')) AS search_key
    FROM public_catalog_facets
    WHERE reference_count > 0 AND char_length(name) BETWEEN 1 AND 40
      AND name = btrim(name) AND name !~ '[[:cntrl:]]'
) AS visible
WHERE char_length(search_key) BETWEEN 1 AND 80
ORDER BY kind,search_key,name
ON CONFLICT(kind,search_key) DO NOTHING;

-- Backfill links for existing books only when a public canonical term exists.
-- Source arrays are not changed: legacy catalogue and recommendation reads
-- continue to operate through their established TEXT[]/facets indexes.
WITH old_labels AS (
    SELECT b.id AS book_id, f.kind,
           lower(replace(btrim(item.label),'ё','е')) AS search_key,
           min(item.ordinal) AS first_position
    FROM books b
    CROSS JOIN LATERAL (
        VALUES ('genre'::TEXT,b.genres),
               ('tag'::TEXT,b.tags),
               ('fandom'::TEXT,b.fandoms)
    ) AS f(kind,labels)
    CROSS JOIN LATERAL unnest(f.labels)
        WITH ORDINALITY AS item(label,ordinal)
    GROUP BY b.id,f.kind,lower(replace(btrim(item.label),'ё','е'))
), matched AS (
    SELECT l.book_id,l.kind,t.id AS term_id,l.first_position
    FROM old_labels l JOIN book_taxonomy_terms t
      ON t.kind=l.kind AND t.search_key=l.search_key AND t.status='active'
), numbered AS (
    SELECT book_id,kind,term_id,
           (row_number() OVER (
              PARTITION BY book_id,kind ORDER BY first_position,term_id
           ) - 1)::SMALLINT AS position
    FROM matched
)
INSERT INTO book_taxonomy_links(book_id,kind,term_id,position)
SELECT book_id,kind,term_id,position FROM numbered WHERE position < 8
ON CONFLICT DO NOTHING;

REVOKE ALL ON book_taxonomy_terms,book_taxonomy_aliases,book_taxonomy_links FROM PUBLIC;
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname='tor_app') THEN
        GRANT SELECT ON book_taxonomy_terms,book_taxonomy_aliases TO tor_app;
        GRANT SELECT,INSERT ON book_taxonomy_links TO tor_app;
    END IF;
END
$$;
