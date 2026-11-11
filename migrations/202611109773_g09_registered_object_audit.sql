-- G-09/S3-RECON-01: reverse PG -> private S3 audit metadata only.
-- This migration never changes storage_objects, version records or S3 content.
CREATE TABLE g09_registered_object_scans (
    bucket TEXT NOT NULL CHECK (
        bucket ~ '^[a-z][a-z0-9-]{1,61}[a-z0-9]$'
        AND bucket NOT LIKE 'public%'
    ),
    scope TEXT NOT NULL CHECK (scope IN ('chapter','chapter_asset')),
    cycle BIGINT NOT NULL DEFAULT 1 CHECK (cycle > 0),
    cursor_id UUID,
    expected BIGINT NOT NULL DEFAULT 0 CHECK (expected >= 0),
    lease_epoch BIGINT NOT NULL DEFAULT 0 CHECK (lease_epoch >= 0),
    leased_until TIMESTAMPTZ,
    scanned BIGINT NOT NULL DEFAULT 0 CHECK (scanned >= 0),
    has_ref BIGINT NOT NULL DEFAULT 0 CHECK (has_ref >= 0),
    healthy BIGINT NOT NULL DEFAULT 0 CHECK (healthy >= 0),
    missing BIGINT NOT NULL DEFAULT 0 CHECK (missing >= 0),
    referenced_missing BIGINT NOT NULL DEFAULT 0 CHECK (referenced_missing >= 0),
    mismatch BIGINT NOT NULL DEFAULT 0 CHECK (mismatch >= 0),
    recently_pending BIGINT NOT NULL DEFAULT 0 CHECK (recently_pending >= 0),
    unknown_attempts BIGINT NOT NULL DEFAULT 0 CHECK (unknown_attempts >= 0),
    retries BIGINT NOT NULL DEFAULT 0 CHECK (retries >= 0),
    consecutive_failures INTEGER NOT NULL DEFAULT 0
        CHECK (consecutive_failures >= 0),
    exhausted BOOLEAN NOT NULL DEFAULT FALSE,
    completed BOOLEAN NOT NULL DEFAULT FALSE,
    checkpoint_at TIMESTAMPTZ,
    PRIMARY KEY (bucket,scope)
);
CREATE TABLE g09_registered_object_findings (
    bucket TEXT NOT NULL,
    scope TEXT NOT NULL CHECK (scope IN ('chapter','chapter_asset')),
    cycle BIGINT NOT NULL CHECK (cycle > 0),
    key_sha256 BYTEA NOT NULL CHECK (octet_length(key_sha256) = 32),
    status TEXT NOT NULL CHECK (status IN (
        'healthy','referenced_but_missing','unreferenced_missing',
        'size_or_sha_mismatch','recently_pending'
    )),
    object_state TEXT NOT NULL
        CHECK (object_state IN ('pending','ready','attached','deleting')),
    has_ref BOOLEAN NOT NULL,
    size_bytes BIGINT NOT NULL CHECK (size_bytes >= 17),
    observed_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (bucket,scope,cycle,key_sha256),
    FOREIGN KEY (bucket,scope)
        REFERENCES g09_registered_object_scans(bucket,scope) ON DELETE RESTRICT
);
CREATE INDEX g09_registered_object_review
    ON g09_registered_object_findings(scope,status,observed_at);
CREATE INDEX storage_objects_g09_reverse_page
    ON storage_objects(bucket,object_kind,id);
