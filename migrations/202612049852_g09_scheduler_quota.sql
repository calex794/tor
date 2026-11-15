-- G-09: cross-replica schedule rate gate; independent of scan leases.
-- Each bucket/scope/direction can be scheduled at most once per 15 minutes.
-- Only diagnostics: never changes ciphertext or reader entitlements.
CREATE TABLE g09_scheduled_pages (
    bucket TEXT NOT NULL CHECK (
        bucket ~ '^[a-z][a-z0-9-]{1,61}[a-z0-9]$'
        AND bucket NOT LIKE 'public%'
    ),
    scope TEXT NOT NULL CHECK (scope IN ('chapter','chapter_asset')),
    direction TEXT NOT NULL CHECK (direction IN ('s3-to-pg','pg-to-s3')),
    next_allowed_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (bucket,scope,direction)
);
