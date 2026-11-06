-- MOD-COVER-01 follow-up: a private original is shared by multiple processing jobs.
-- NEVER retire it merely because one rejected candidate becomes old.
-- Retired source metadata, SHA-256 and all review decisions remain immutable records.
ALTER TABLE cover_upload_objects
    DROP CONSTRAINT cover_upload_objects_status_check;
ALTER TABLE cover_upload_objects
    ADD CONSTRAINT cover_upload_objects_status_check
    CHECK (status IN ('pending','verified','quarantined','retired'));

-- B owns source eligibility. Durable deletion is optional and fenced.
CREATE TABLE cover_private_source_removals (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    source_object_id BIGINT NOT NULL UNIQUE
        REFERENCES cover_upload_objects(id) ON DELETE RESTRICT,
    storage_key TEXT NOT NULL UNIQUE
        CHECK (storage_key LIKE 'staging/covers/sources/%'
               AND char_length(storage_key) BETWEEN 28 AND 512),
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending','leased','removed')),
    lease_epoch BIGINT NOT NULL DEFAULT 0 CHECK (lease_epoch >= 0),
    locked_until TIMESTAMPTZ,
    retry_after TIMESTAMPTZ NOT NULL DEFAULT now(),
    removed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((state='removed') = (removed_at IS NOT NULL))
);
CREATE INDEX cover_private_source_removal_claim
    ON cover_private_source_removals (retry_after, id)
    WHERE state IN ('pending','leased');

-- A source cannot be re-enqueued after entering source-retirement.
-- Repository SELECT FOR SHARE serializes this with the staging transaction.
-- No web route or job worker may write the cleanup ledger directly.
