-- S023: private, verified UTF-8 import source and durable, cancellable chapter jobs.
-- This schema never stores chapter plaintext or a public S3 URL.
CREATE TABLE chapter_import_jobs (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    actor_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    command_key TEXT NOT NULL CHECK (char_length(command_key)=43),
    source_key TEXT NOT NULL UNIQUE
        CHECK (char_length(source_key) BETWEEN 32 AND 512
           AND source_key LIKE 'staging/chapters/%'
           AND position('..' in source_key)=0),
    source_filename TEXT NOT NULL
        CHECK (char_length(source_filename) BETWEEN 1 AND 128
           AND position('/' in source_filename)=0
           AND right(lower(source_filename),4) IN ('.txt','.md')),
    source_sha256 BYTEA NOT NULL CHECK (octet_length(source_sha256)=32),
    source_bytes BIGINT NOT NULL CHECK (source_bytes BETWEEN 1 AND 700000),
    approved_fingerprint BYTEA NOT NULL CHECK (octet_length(approved_fingerprint)=32),
    expected_chapter_count INTEGER NOT NULL CHECK (expected_chapter_count BETWEEN 1 AND 500),
    status TEXT NOT NULL DEFAULT 'queued'
        CHECK (status IN ('queued','running','succeeded','failed','cancelled')),
    lease_epoch BIGINT NOT NULL DEFAULT 0 CHECK (lease_epoch>=0),
    locked_until TIMESTAMPTZ,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 8),
    run_after TIMESTAMPTZ NOT NULL DEFAULT now(),
    saved_count INTEGER NOT NULL DEFAULT 0 CHECK (saved_count BETWEEN 0 AND 500),
    last_error_code TEXT CHECK (char_length(last_error_code) BETWEEN 1 AND 100),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    UNIQUE (actor_user_id,command_key),
    CHECK ((status='running')=(locked_until IS NOT NULL)),
    CHECK (status<>'succeeded' OR saved_count=expected_chapter_count)
);
-- Concurrent submissions cannot create two active jobs for one book.
CREATE UNIQUE INDEX chapter_import_one_active_book
    ON chapter_import_jobs(book_id)
    WHERE status IN ('queued','running');
CREATE INDEX chapter_import_claim
    ON chapter_import_jobs(run_after,id)
    WHERE status IN ('queued','running');
CREATE INDEX chapter_import_owner_status
    ON chapter_import_jobs(book_id,actor_user_id,id DESC);

COMMENT ON TABLE chapter_import_jobs IS
    'S023: verified canonical UTF-8 chapter source in private S3; final B chapter import and job success must commit together.';
