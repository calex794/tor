-- S349: fresh-install-only change. No historical chapter bodies exist in TOR.
-- New S3-backed editions reserve IDs without temporarily writing Markdown.
-- Previously applied migration files remain immutable per I governance.
ALTER TABLE chapter_versions ALTER COLUMN body_markdown DROP NOT NULL;

-- Refuse to expose an S3-only edition through the publication table unless
-- the exact immutable free-storage reference is attached in the same commit.
-- Legacy PostgreSQL-backed fixtures still pass until all B/C tests are switched.
CREATE FUNCTION guard_s349_free_publication()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM chapter_versions v
        JOIN chapters c ON c.id=v.chapter_id
        WHERE v.id=NEW.version_id AND v.chapter_id=NEW.chapter_id
          AND c.access_level='free' AND v.body_markdown IS NULL
          AND NOT EXISTS (
             SELECT 1 FROM free_chapter_storage_objects f
             WHERE f.chapter_id=v.chapter_id AND f.version_id=v.id
          )
    ) THEN
        RAISE EXCEPTION 'S349: free S3 edition requires verified immutable storage'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER s349_guard_free_publication
    BEFORE INSERT OR UPDATE ON chapter_publications
    FOR EACH ROW EXECUTE FUNCTION guard_s349_free_publication();
