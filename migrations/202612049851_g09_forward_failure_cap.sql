-- G-09: preserve a durable error budget for the S3 -> PostgreSQL LIST audit.
-- Fail-closed after eight consecutive provider failures; the operator must
-- explicitly resume an exhausted scan after investigating the S3 incident.
-- No changes to storage_objects, chapter_versions, entitlements or S3 data.
ALTER TABLE g09_private_inventory_scans
    ADD COLUMN retries BIGINT NOT NULL DEFAULT 0 CHECK (retries >= 0),
    ADD COLUMN unknown_attempts BIGINT NOT NULL DEFAULT 0 CHECK (unknown_attempts >= 0),
    ADD COLUMN consecutive_failures INTEGER NOT NULL DEFAULT 0
        CHECK (consecutive_failures >= 0),
    ADD COLUMN exhausted BOOLEAN NOT NULL DEFAULT FALSE;
