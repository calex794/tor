-- Original DROP-04a: only a VERIFIED, private PACK upload receipt is staged.
-- The source batch has reserved immutable chapter/version IDs, but no chapter
-- publication, SINGLE object, active PACK pointer or paid content is created.
-- This migration MUST NOT run during web startup or on the live VPS.
CREATE TABLE free_pack_direct_import_stages (
    job_id BIGINT PRIMARY KEY REFERENCES chapter_import_jobs(id) ON DELETE RESTRICT,
    actor_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    approved_fingerprint BYTEA NOT NULL CHECK (octet_length(approved_fingerprint)=32),
    chapter_count INTEGER NOT NULL CHECK (chapter_count BETWEEN 1 AND 500),
    bucket TEXT NOT NULL CHECK (
        bucket ~ '^[a-z][a-z0-9-]{1,61}[a-z0-9]$' AND bucket NOT LIKE 'public%'
    ),
    object_key TEXT NOT NULL UNIQUE CHECK (
        char_length(object_key) BETWEEN 32 AND 240
        AND object_key ~ '^private/free-packs/import/[A-Za-z0-9_./-]+[.]zst$'
        AND position('//' in object_key)=0 AND object_key !~ '(^|/)[.][.]?(/|$)'
    ),
    pack_sha256 BYTEA NOT NULL CHECK (octet_length(pack_sha256)=32),
    manifest_sha256 BYTEA NOT NULL CHECK (octet_length(manifest_sha256)=32),
    receipt JSONB NOT NULL CHECK (
        jsonb_typeof(receipt)='object' AND receipt ? 'target' AND receipt ? 'frames'
    ),
    approved_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE TRIGGER free_pack_direct_stages_immutable
    BEFORE UPDATE OR DELETE ON free_pack_direct_import_stages FOR EACH ROW
    EXECUTE FUNCTION reject_free_chapter_storage_change();
CREATE INDEX free_pack_direct_stages_book_idx
    ON free_pack_direct_import_stages(book_id,job_id);

COMMENT ON TABLE free_pack_direct_import_stages IS
    'DROP-04a: immutable verified S3 PACK proof for a leased import batch; NOT chapter publication, entitlement or reader placement.';
