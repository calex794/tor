-- G-09 / S3-RECON-01 (#748): operator-only S3 inventory checkpoint.
-- Records observations; NOTHING in this migration authorizes S3 deletion.
CREATE TABLE storage_inventory_scans (
    bucket TEXT NOT NULL,
    prefix TEXT NOT NULL CHECK (
        prefix IN ('private/chapter-versions/', 'private/chapter-assets/')
    ),
    continuation_token TEXT,
    cycle BIGINT NOT NULL DEFAULT 1 CHECK (cycle > 0),
    lease_epoch BIGINT NOT NULL DEFAULT 0 CHECK (lease_epoch >= 0),
    lease_until TIMESTAMPTZ,
    scanned_count BIGINT NOT NULL DEFAULT 0 CHECK (scanned_count >= 0),
    registered_count BIGINT NOT NULL DEFAULT 0 CHECK (registered_count >= 0),
    unknown_count BIGINT NOT NULL DEFAULT 0 CHECK (unknown_count >= 0),
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (bucket, prefix),
    CHECK (registered_count + unknown_count = scanned_count)
);

-- Candidate keys are NOT a deletion queue. Even repeated observations cannot
-- give a worker permission to DELETE a purchased or other stored object.
CREATE TABLE storage_inventory_candidates (
    bucket TEXT NOT NULL,
    prefix TEXT NOT NULL,
    object_key TEXT NOT NULL,
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_cycle BIGINT NOT NULL CHECK (last_cycle > 0),
    observations BIGINT NOT NULL DEFAULT 1 CHECK (observations > 0),
    PRIMARY KEY (bucket, prefix, object_key),
    FOREIGN KEY (bucket, prefix)
       REFERENCES storage_inventory_scans(bucket, prefix) ON DELETE RESTRICT,
    CHECK (object_key LIKE prefix || '%')
);

CREATE INDEX storage_inventory_candidates_by_cycle_idx
    ON storage_inventory_candidates(bucket, prefix, last_cycle);
