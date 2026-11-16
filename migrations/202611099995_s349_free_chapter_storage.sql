-- I/B/G S349: first additive S3-only free chapter storage metadata.
-- This does NOT switch the reader or publisher and does not alter paid G objects.
-- Old body_markdown stays untouched until S3 migration is verified.
CREATE TABLE free_chapter_storage_objects (
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL,
    bucket TEXT NOT NULL CHECK (
        bucket ~ '^[a-z][a-z0-9-]{1,61}[a-z0-9]$'
        AND bucket NOT LIKE 'public%'
    ),
    object_key TEXT NOT NULL UNIQUE,
    sha256 BYTEA NOT NULL CHECK (octet_length(sha256)=32),
    size_bytes BIGINT NOT NULL CHECK (size_bytes BETWEEN 1 AND 200000),
    content_type TEXT NOT NULL DEFAULT 'text/plain; charset=utf-8'
        CHECK (content_type='text/plain; charset=utf-8'),
    verified_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (chapter_id,version_id),
    CONSTRAINT free_chapter_storage_version_fk
        FOREIGN KEY (chapter_id,version_id)
        REFERENCES chapter_versions(chapter_id,id) ON DELETE RESTRICT,
    CONSTRAINT free_chapter_storage_key_matches_version CHECK (
        object_key='private/free-chapters/'||chapter_id::text||'/'||version_id::text||'.md'
    )
);

-- The publisher may attach ONLY an existing FREE edition with its exact B
-- SHA. This trigger checks metadata consistency, not real S3 GET evidence;
-- that receipt is provided by G during the future B/G attachment operation.
CREATE FUNCTION guard_free_chapter_storage_insert()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM chapter_versions v
        JOIN chapters c ON c.id=v.chapter_id AND c.access_level='free'
        WHERE v.id=NEW.version_id AND v.chapter_id=NEW.chapter_id
          AND v.content_sha256 IS NOT NULL
          AND v.content_sha256=NEW.sha256
    ) THEN
        RAISE EXCEPTION 'free storage requires exact free edition and content hash'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER free_chapter_storage_verified_edition
    BEFORE INSERT ON free_chapter_storage_objects
    FOR EACH ROW EXECUTE FUNCTION guard_free_chapter_storage_insert();

-- Never change an attached edition's key or digest in place. Version
-- retirement will need a separate authorized contract; no implicit GC.
CREATE FUNCTION reject_free_chapter_storage_change()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'immutable free chapter storage reference'
        USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER free_chapter_storage_immutable
    BEFORE UPDATE OR DELETE ON free_chapter_storage_objects
    FOR EACH ROW EXECUTE FUNCTION reject_free_chapter_storage_change();

COMMENT ON TABLE free_chapter_storage_objects IS
    'S349: private immutable free text S3 reference only; not publication or access grant';
