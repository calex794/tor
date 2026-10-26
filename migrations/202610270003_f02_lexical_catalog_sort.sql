-- F-02/S001: canonical public keyset indices for NEwBOO-style lexical sorts.
-- Publication filtering is performed before pagination. The id breaks ties.
CREATE INDEX books_public_title_keyset_idx
    ON books ((lower(title)), id)
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted;

CREATE INDEX books_public_author_keyset_idx
    ON books ((lower(author_name)), id)
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted;
