-- S020/B: one canonical row represents an undirected NEwBOO related-book edge.
-- Only an authenticated owner may mutate edges through B. Public target
-- eligibility and no-self rules are enforced again in PostgreSQL transactions.
CREATE TABLE book_related_links (
    smaller_book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    larger_book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (smaller_book_id, larger_book_id),
    CONSTRAINT book_related_links_no_self CHECK (smaller_book_id < larger_book_id)
);
CREATE INDEX book_related_links_large_idx ON book_related_links (larger_book_id, smaller_book_id);
-- Runtime grants are provided by the tor_migrator default privileges.
-- The generic migration verifier intentionally has no tor_app role.
-- Exact main-title lookup for public related-book suggestions, across languages.
-- Restricted to public candidates to avoid leaking private draft names.
CREATE INDEX books_related_public_exact_title_idx
    ON books (book_registration_legacy_title_key(title), id)
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted;
