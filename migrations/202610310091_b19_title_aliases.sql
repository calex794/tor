-- B-19: add a bounded indexed advisory match before book creation.
-- Source/language uniqueness in books remains the only canonical hard guard.
-- The aliases of a hidden book are NEVER disclosed except to its owner.
CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA public;

CREATE TABLE book_title_aliases (
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('main','original','alternative')),
    title TEXT NOT NULL CHECK (title = btrim(title) AND char_length(title) BETWEEN 1 AND 200),
    normalized_title TEXT GENERATED ALWAYS AS (
        translate(lower(normalize(title, NFKC)), 'ё', 'е')
    ) STORED,
    PRIMARY KEY (book_id, kind, title)
);
CREATE INDEX book_title_aliases_normalized_exact ON book_title_aliases(normalized_title);
CREATE INDEX book_title_aliases_normalized_fuzzy
    ON book_title_aliases USING GIN (normalized_title gin_trgm_ops);

-- Backfill legacy books; include all book states without making them public.
INSERT INTO book_title_aliases (book_id, kind, title)
SELECT id, 'main', title FROM books;

-- All future main-title inserts/updates remain consistent, including SQL import.
-- SECURITY DEFINER is necessary because app writers cannot delete aliases.
CREATE FUNCTION public.b19_sync_main_title_alias()
RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, public, pg_temp AS $$
BEGIN
    DELETE FROM public.book_title_aliases WHERE book_id = NEW.id AND kind = 'main';
    INSERT INTO public.book_title_aliases(book_id, kind, title)
    VALUES (NEW.id, 'main', NEW.title);
    RETURN NEW;
END;
$$;
CREATE TRIGGER b19_main_title_after_write
AFTER INSERT OR UPDATE OF title ON books
FOR EACH ROW EXECUTE FUNCTION public.b19_sync_main_title_alias();
REVOKE ALL ON FUNCTION public.b19_sync_main_title_alias() FROM PUBLIC;

-- Originals and alternatives are inserted only as part of the B-owned book
-- creation transaction; runtime cannot UPDATE/DELETE existing alias rows.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'tor_app') THEN
        REVOKE UPDATE, DELETE, TRUNCATE ON public.book_title_aliases FROM tor_app;
        GRANT SELECT, INSERT ON public.book_title_aliases TO tor_app;
    END IF;
END;
$$;
