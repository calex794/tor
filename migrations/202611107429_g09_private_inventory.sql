-- G-09/S3-RECON-01: read-only inventory checkpoint and sanitized findings.
-- Never delete or mutate storage_objects, chapter_versions, or S3 objects.
CREATE TABLE g09_private_inventory_scans (
    bucket TEXT NOT NULL CHECK (length(bucket) BETWEEN 3 AND 63),
    scope TEXT NOT NULL CHECK (scope IN ('chapter', 'chapter_asset')),
    cycle BIGINT NOT NULL DEFAULT 1 CHECK (cycle > 0),
    continuation TEXT,
    lease_epoch BIGINT NOT NULL DEFAULT 0 CHECK (lease_epoch >= 0),
    leased_until TIMESTAMPTZ,
    scanned BIGINT NOT NULL DEFAULT 0 CHECK (scanned >= 0),
    registered BIGINT NOT NULL DEFAULT 0 CHECK (registered >= 0),
    recently_pending BIGINT NOT NULL DEFAULT 0 CHECK (recently_pending >= 0),
    unregistered_candidates BIGINT NOT NULL DEFAULT 0
        CHECK (unregistered_candidates >= 0),
    size_mismatch BIGINT NOT NULL DEFAULT 0 CHECK (size_mismatch >= 0),
    unknown BIGINT NOT NULL DEFAULT 0 CHECK (unknown >= 0),
    completed BOOLEAN NOT NULL DEFAULT FALSE,
    checkpoint_at TIMESTAMPTZ,
    PRIMARY KEY (bucket, scope)
);

-- No raw S3 paths or opaque continuation tokens in per-object findings.
CREATE TABLE g09_private_inventory_findings (
    bucket TEXT NOT NULL,
    scope TEXT NOT NULL CHECK (scope IN ('chapter', 'chapter_asset')),
    cycle BIGINT NOT NULL CHECK (cycle > 0),
    key_sha256 BYTEA NOT NULL CHECK (octet_length(key_sha256) = 32),
    status TEXT NOT NULL CHECK (status IN (
        'registered', 'recently_pending', 'unregistered_candidate',
        'size_mismatch', 'unknown'
    )),
    size_bytes BIGINT CHECK (size_bytes >= 0),
    observed_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (bucket, scope, cycle, key_sha256),
    FOREIGN KEY (bucket, scope)
        REFERENCES g09_private_inventory_scans(bucket, scope) ON DELETE RESTRICT
);
CREATE INDEX g09_private_inventory_review
    ON g09_private_inventory_findings(scope, status, observed_at);
