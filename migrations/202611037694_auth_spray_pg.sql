-- AB-02: independent, bounded, shared password-spray state.
-- Both keys are 32-byte keyed HMACs produced by the application. Never store
-- plaintext IP, subnet, email, password, or a session/cookie value here.
-- Subnet rows are diagnostic only; a subnet never acquires a login lock.
CREATE TABLE auth_spray_windows (
    scope TEXT NOT NULL CHECK (scope IN ('ip', 'subnet')),
    source_key BYTEA NOT NULL CHECK (octet_length(source_key) = 32),
    period_started TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    touched_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    failed_attempts INTEGER NOT NULL DEFAULT 0 CHECK (failed_attempts >= 0),
    distinct_targets INTEGER NOT NULL DEFAULT 0 CHECK (distinct_targets >= 0),
    locked_until TIMESTAMPTZ,
    PRIMARY KEY (scope, source_key),
    CHECK (scope = 'ip' OR locked_until IS NULL)
);
CREATE INDEX auth_spray_windows_retention_idx ON auth_spray_windows (touched_at);

-- Unique targets are serialized by the matching window's FOR UPDATE lock.
-- These rows are automatically purged on window reset and retention cleanup.
CREATE TABLE auth_spray_targets (
    scope TEXT NOT NULL,
    source_key BYTEA NOT NULL,
    target_key BYTEA NOT NULL CHECK (octet_length(target_key) = 32),
    PRIMARY KEY (scope, source_key, target_key),
    FOREIGN KEY (scope, source_key)
        REFERENCES auth_spray_windows(scope, source_key) ON DELETE CASCADE
);
