-- G-09/S3-RECON-01: opt-in bounded ciphertext GET/SHA-256 scan metadata.
-- Only these G-09-owned tables may be written by the body audit.
-- No object identifiers, raw S3 paths, credentials or ciphertext in findings.
CREATE TABLE g09_body_sha_scans (
    bucket TEXT NOT NULL CHECK (
        bucket ~ '^[a-z][a-z0-9-]{1,61}[a-z0-9]$'
        AND bucket NOT LIKE 'public%'
    ),
    scope TEXT NOT NULL CHECK (scope IN ('chapter', 'chapter_asset')),
    cycle BIGINT NOT NULL DEFAULT 1 CHECK (cycle > 0),
    cursor_id UUID,
    expected BIGINT NOT NULL DEFAULT 0 CHECK (expected >= 0),
    lease_epoch BIGINT NOT NULL DEFAULT 0 CHECK (lease_epoch >= 0),
    leased_until TIMESTAMPTZ,
    scanned BIGINT NOT NULL DEFAULT 0 CHECK (scanned >= 0),
    verified BIGINT NOT NULL DEFAULT 0 CHECK (verified >= 0),
    missing BIGINT NOT NULL DEFAULT 0 CHECK (missing >= 0),
    body_mismatch BIGINT NOT NULL DEFAULT 0 CHECK (body_mismatch >= 0),
    metadata_mismatch BIGINT NOT NULL DEFAULT 0 CHECK (metadata_mismatch >= 0),
    skipped BIGINT NOT NULL DEFAULT 0 CHECK (skipped >= 0),
    unknown_attempts BIGINT NOT NULL DEFAULT 0 CHECK (unknown_attempts >= 0),
    retries BIGINT NOT NULL DEFAULT 0 CHECK (retries >= 0),
    consecutive_failures INTEGER NOT NULL DEFAULT 0 CHECK (consecutive_failures >= 0),
    exhausted BOOLEAN NOT NULL DEFAULT FALSE,
    completed BOOLEAN NOT NULL DEFAULT FALSE,
    checkpoint_at TIMESTAMPTZ,
    PRIMARY KEY (bucket,scope)
);

CREATE TABLE g09_body_sha_findings (
    bucket TEXT NOT NULL,
    scope TEXT NOT NULL CHECK (scope IN ('chapter', 'chapter_asset')),
    cycle BIGINT NOT NULL CHECK (cycle > 0),
    key_sha256 BYTEA NOT NULL CHECK (octet_length(key_sha256) = 32),
    status TEXT NOT NULL CHECK (status IN (
        'missing','body_mismatch','metadata_mismatch'
    )),
    size_bytes BIGINT NOT NULL CHECK (size_bytes >= 17),
    observed_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (bucket,scope,cycle,key_sha256),
    FOREIGN KEY (bucket,scope)
        REFERENCES g09_body_sha_scans(bucket,scope) ON DELETE RESTRICT
);
CREATE INDEX g09_body_sha_review
    ON g09_body_sha_findings(scope,status,observed_at);
