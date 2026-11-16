-- Import object identities are reserved without creating live chapters.
-- Cancellation/lease loss can leave only private provisional object metadata.
CREATE TABLE free_chapter_import_reservations (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    command_key TEXT NOT NULL CHECK (command_key ~ '^[A-Za-z0-9_-]{43}$'),
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    chapter_index INTEGER NOT NULL CHECK (chapter_index BETWEEN 0 AND 499),
    chapter_id BIGINT NOT NULL UNIQUE CHECK (chapter_id>0),
    version_id BIGINT NOT NULL UNIQUE CHECK (version_id>0),
    chapter_no INTEGER NOT NULL CHECK (chapter_no BETWEEN 1 AND 1000000),
    title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 200),
    batch_sha256 BYTEA NOT NULL CHECK (octet_length(batch_sha256)=32),
    content_sha256 BYTEA NOT NULL CHECK (octet_length(content_sha256)=32),
    size_bytes BIGINT NOT NULL CHECK (size_bytes BETWEEN 1 AND 200000),
    block_utf16_lengths INTEGER[] NOT NULL CHECK (cardinality(block_utf16_lengths) BETWEEN 1 AND 200),
    last_readable_block_index INTEGER NOT NULL CHECK (last_readable_block_index BETWEEN 0 AND 200),
    last_readable_char_offset INTEGER NOT NULL CHECK (last_readable_char_offset BETWEEN 0 AND 100000),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (user_id,command_key,chapter_index)
);
CREATE TRIGGER free_import_reservation_immutable
    BEFORE UPDATE OR DELETE ON free_chapter_import_reservations
    FOR EACH ROW EXECUTE FUNCTION reject_free_chapter_storage_change();

CREATE TABLE free_chapter_import_uploads (
    version_id BIGINT PRIMARY KEY REFERENCES free_chapter_import_reservations(version_id) ON DELETE RESTRICT,
    bucket TEXT NOT NULL CHECK (bucket ~ '^[a-z][a-z0-9-]{1,61}[a-z0-9]$' AND bucket NOT LIKE 'public%'),
    content_sha256 BYTEA NOT NULL CHECK (octet_length(content_sha256)=32),
    size_bytes BIGINT NOT NULL CHECK (size_bytes BETWEEN 1 AND 200000),
    verified_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE TRIGGER free_import_upload_immutable
    BEFORE UPDATE OR DELETE ON free_chapter_import_uploads
    FOR EACH ROW EXECUTE FUNCTION reject_free_chapter_storage_change();
