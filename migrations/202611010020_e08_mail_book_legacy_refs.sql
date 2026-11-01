-- E-08/S057: explicit, reviewed legacy identifiers used only by mail previews.
-- The canonical web URL remains /books/{numeric_id}; never infer aliases from
-- source_identity (a work+language key) or scan hidden books by name.
CREATE TABLE book_legacy_mail_refs (
    alias_key TEXT PRIMARY KEY
        CHECK (alias_key ~ '^[a-z][a-z0-9_.-]{0,127}$'
               AND alias_key = lower(alias_key)),
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE CASCADE
);
CREATE INDEX book_legacy_mail_refs_by_book ON book_legacy_mail_refs(book_id);
COMMENT ON TABLE book_legacy_mail_refs IS
    'Explicit NEwBOO legacy alias -> canonical Book mapping; populate only after verified one-to-one import, never by guessing.';
