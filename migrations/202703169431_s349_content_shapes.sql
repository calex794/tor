-- Immutable reader coordinates computed by B from the exact canonical source.
-- No text, excerpts, signed URLs or reader identity are stored here.
CREATE TABLE chapter_version_content_shapes (
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL,
    content_sha256 BYTEA NOT NULL CHECK (octet_length(content_sha256)=32),
    block_utf16_lengths INTEGER[] NOT NULL,
    last_readable_block_index INTEGER NOT NULL,
    last_readable_char_offset INTEGER NOT NULL,
    PRIMARY KEY (chapter_id,version_id),
    FOREIGN KEY (chapter_id,version_id)
        REFERENCES chapter_versions(chapter_id,id) ON DELETE RESTRICT,
    CHECK (cardinality(block_utf16_lengths) BETWEEN 1 AND 200
        AND array_ndims(block_utf16_lengths)=1
        AND array_lower(block_utf16_lengths,1)=1
        AND array_position(block_utf16_lengths,NULL) IS NULL
        AND 0 <= ALL(block_utf16_lengths) AND 100000 >= ALL(block_utf16_lengths)
        AND last_readable_block_index BETWEEN 0 AND cardinality(block_utf16_lengths)
        AND ((last_readable_block_index=0 AND last_readable_char_offset=0)
          OR (last_readable_block_index>0
            AND last_readable_char_offset=block_utf16_lengths[last_readable_block_index]))));

CREATE FUNCTION guard_chapter_content_shape() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM chapter_versions v
        WHERE v.chapter_id=NEW.chapter_id AND v.id=NEW.version_id
          AND v.content_sha256=NEW.content_sha256) THEN
        RAISE EXCEPTION 'content shape must match immutable edition' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER chapter_content_shape_verified BEFORE INSERT ON chapter_version_content_shapes
    FOR EACH ROW EXECUTE FUNCTION guard_chapter_content_shape();
CREATE TRIGGER chapter_content_shape_immutable BEFORE UPDATE OR DELETE ON chapter_version_content_shapes
    FOR EACH ROW EXECUTE FUNCTION reject_free_chapter_storage_change();
