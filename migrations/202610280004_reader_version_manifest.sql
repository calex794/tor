-- B-04/C-06/S3 ReaderVersionManifest v1, never text or reader entitlement.
-- 202610280003 is reserved by S3-34 KEK rotation PR #148.
-- No old migration is edited. Historical attached objects require explicit
-- B-owned backfill through the same verified attachment path.
CREATE TABLE chapter_reader_manifests (
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL,
    storage_object_id UUID NOT NULL REFERENCES storage_objects(id) ON DELETE RESTRICT,
    parser_schema SMALLINT NOT NULL DEFAULT 1 CHECK (parser_schema = 1),
    content_sha256 BYTEA NOT NULL CHECK (octet_length(content_sha256) = 32),
    block_utf16_lengths INTEGER[] NOT NULL,
    last_readable_block_index INTEGER NOT NULL CHECK (last_readable_block_index BETWEEN 1 AND 200),
    last_readable_char_offset INTEGER NOT NULL CHECK (last_readable_char_offset BETWEEN 1 AND 100000),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (chapter_id, version_id),
    FOREIGN KEY (chapter_id, version_id)
        REFERENCES chapter_versions(chapter_id, id) ON DELETE RESTRICT,
    CHECK (
        cardinality(block_utf16_lengths) BETWEEN 1 AND 200
        AND array_ndims(block_utf16_lengths) = 1
        AND array_lower(block_utf16_lengths, 1) = 1
        AND array_position(block_utf16_lengths, NULL) IS NULL
        AND last_readable_block_index <= cardinality(block_utf16_lengths)
        AND last_readable_char_offset = block_utf16_lengths[last_readable_block_index]
    )
);

CREATE FUNCTION require_verified_reader_manifest()
RETURNS trigger LANGUAGE plpgsql AS $reader_manifest_verified$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM chapter_versions AS v
        JOIN storage_objects AS o ON o.id=v.storage_object_id
        WHERE v.chapter_id=NEW.chapter_id AND v.id=NEW.version_id
          AND v.storage_object_id=NEW.storage_object_id
          AND o.object_kind='chapter' AND o.codec='gzip'
          AND o.state='attached'
          AND o.plaintext_sha256=NEW.content_sha256
          AND o.plaintext_sha256=sha256(convert_to(
              replace(replace(v.body_markdown,E'\r\n',E'\n'),E'\r',E'\n'), 'UTF8'
          ))
    ) THEN
        RAISE EXCEPTION 'reader manifest must match verified immutable encrypted version'
            USING ERRCODE='23514';
    END IF;
    IF EXISTS (
        SELECT 1 FROM unnest(NEW.block_utf16_lengths) AS b(len)
        WHERE b.len < 0 OR b.len > 100000
    ) THEN
        RAISE EXCEPTION 'invalid reader manifest block length'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$reader_manifest_verified$;

CREATE TRIGGER reader_manifest_verified
    BEFORE INSERT ON chapter_reader_manifests
    FOR EACH ROW EXECUTE FUNCTION require_verified_reader_manifest();

CREATE FUNCTION immutable_reader_manifest()
RETURNS trigger LANGUAGE plpgsql AS $reader_manifest_immutable$
BEGIN
    RAISE EXCEPTION 'reader manifest is immutable'
        USING ERRCODE='23514';
END;
$reader_manifest_immutable$;

CREATE TRIGGER reader_manifest_immutable
    BEFORE UPDATE OR DELETE ON chapter_reader_manifests
    FOR EACH ROW EXECUTE FUNCTION immutable_reader_manifest();
