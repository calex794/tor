-- AUTH-READ-02b: rebuildable, bounded derivative of an exact FREE edition.
-- This does not activate guest mode or change the publication/retention model.
CREATE TABLE chapter_version_guest_prefixes (
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL,
    algorithm_version TEXT NOT NULL CHECK (algorithm_version='GUEST-PREFIX-1'),
    source_sha256 BYTEA NOT NULL CHECK (octet_length(source_sha256)=32),
    visible_scalar_count INTEGER NOT NULL CHECK (visible_scalar_count BETWEEN 0 AND 50000),
    prefix_text TEXT NOT NULL,
    prefix_sha256 BYTEA NOT NULL CHECK (octet_length(prefix_sha256)=32),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (chapter_id,version_id,algorithm_version),
    FOREIGN KEY (chapter_id,version_id)
        REFERENCES chapter_versions(chapter_id,id) ON DELETE CASCADE,
    CHECK (char_length(prefix_text) <= LEAST(
        2000, visible_scalar_count / 2, GREATEST(visible_scalar_count-1,0))),
    CHECK (octet_length(prefix_text) <= 8000),
    CHECK (prefix_sha256=sha256(convert_to(prefix_text,'UTF8')))
);

-- The writer must derive prefix_text from verified source bytes. This trigger
-- additionally enforces exact identity/current FREE publication and source hash.
-- Legacy PG bytes are inspected only on preparation, never on a guest read.
CREATE FUNCTION auth_read_guard_guest_prefix() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM chapter_versions v
        JOIN chapters c ON c.id=v.chapter_id
        JOIN chapter_publications p ON p.chapter_id=c.id AND p.version_id=v.id
        WHERE v.chapter_id=NEW.chapter_id AND v.id=NEW.version_id
          AND c.access_level='free'
          AND (
            (v.body_markdown IS NULL AND v.content_sha256=NEW.source_sha256)
            OR (v.body_markdown IS NOT NULL
                AND sha256(convert_to(v.body_markdown,'UTF8'))=NEW.source_sha256
                AND (v.content_sha256 IS NULL OR v.content_sha256=NEW.source_sha256))
          )
    ) THEN
        RAISE EXCEPTION 'guest prefix requires exact current FREE source'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER auth_read_guard_guest_prefix
    BEFORE INSERT ON chapter_version_guest_prefixes
    FOR EACH ROW EXECUTE FUNCTION auth_read_guard_guest_prefix();

-- An exact derivative may be deleted and regenerated (or cascade with an
-- already-authorized physical version deletion), but never rewritten in place.
CREATE FUNCTION auth_read_guest_prefix_no_rewrite() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'guest prefix cannot be rewritten' USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER auth_read_guest_prefix_no_rewrite
    BEFORE UPDATE ON chapter_version_guest_prefixes
    FOR EACH ROW EXECUTE FUNCTION auth_read_guest_prefix_no_rewrite();
