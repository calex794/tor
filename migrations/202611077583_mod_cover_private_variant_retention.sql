-- MOD-COVER-01: durable, opt-in removal of terminal PRIVATE WebP variants.
-- This ledger never deletes review requests, immutable decisions, or source uploads.
-- Only rejected/superseded candidates older than seven days may be staged.
-- Supersession time is recorded at the actual transition, not inferred
-- from initial submission. Legacy superseded rows with unknown age are
-- intentionally ineligible for automatic deletion (fail closed).
ALTER TABLE book_cover_review_requests
    ADD COLUMN superseded_at TIMESTAMPTZ;
ALTER TABLE book_cover_review_requests
    ADD CONSTRAINT cover_review_supersession_time
    CHECK (superseded_at IS NULL OR state='superseded');
CREATE TABLE cover_private_variant_removals (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    job_id BIGINT NOT NULL REFERENCES cover_variant_jobs(id) ON DELETE RESTRICT,
    variant_name TEXT NOT NULL CHECK (variant_name IN ('main', 'card')),
    storage_key TEXT NOT NULL UNIQUE CHECK (
        storage_key LIKE 'staging/covers/variants/%'
        AND char_length(storage_key) BETWEEN 28 AND 512
    ),
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'leased', 'removed')),
    lease_epoch BIGINT NOT NULL DEFAULT 0 CHECK (lease_epoch >= 0),
    locked_until TIMESTAMPTZ,
    retry_after TIMESTAMPTZ NOT NULL DEFAULT now(),
    removed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (job_id, variant_name),
    CHECK ((state='removed') = (removed_at IS NOT NULL))
);
CREATE INDEX cover_private_variant_removal_claim
    ON cover_private_variant_removals (retry_after, id)
    WHERE state IN ('pending','leased');
