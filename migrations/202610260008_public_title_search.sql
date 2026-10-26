-- F-03: public-only GIN index. Same expression and predicate as repository query.
CREATE INDEX books_public_title_search_idx
ON books USING GIN (to_tsvector('simple'::regconfig, title))
WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted;
