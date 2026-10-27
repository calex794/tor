-- G-09 / S3-12: leased cleanup for orphaned PRIVATE ciphertext only.
-- A paid attached version can never be selected, even when delete_after
-- erroneously becomes due. No new public API or production scheduler.
ALTER TABLE storage_objects
    ADD COLUMN gc_lease_epoch BIGINT NOT NULL DEFAULT 0
        CHECK (gc_lease_epoch >= 0),
    ADD COLUMN gc_attempts INTEGER NOT NULL DEFAULT 0
        CHECK (gc_attempts BETWEEN 0 AND 8),
    ADD COLUMN gc_locked_until TIMESTAMPTZ,
    ADD COLUMN gc_retry_after TIMESTAMPTZ,
    ADD COLUMN gc_error_code TEXT
        CHECK (gc_error_code IN ('unavailable','corrupt','conflict','invalid')),
    ADD CONSTRAINT gc_lease_only_during_delete CHECK (
        gc_locked_until IS NULL OR state='deleting'
    ),
    ADD CONSTRAINT gc_retry_only_during_delete CHECK (
        gc_retry_after IS NULL OR state='deleting'
    ),
    ADD CONSTRAINT gc_attached_never_leased CHECK (
        state<>'attached'
        OR (gc_lease_epoch=0 AND gc_attempts=0
            AND gc_locked_until IS NULL AND gc_retry_after IS NULL
            AND gc_error_code IS NULL)
    );
CREATE INDEX storage_gc_due_lease_idx
    ON storage_objects (delete_after, gc_retry_after, gc_locked_until, id)
    WHERE state IN ('pending','ready','deleting')
      AND delete_after IS NOT NULL AND gc_attempts < 8;

-- A historical B association alone also vetoes physical deletion, even
-- when an operator incorrectly sets the G row to 'ready' or 'deleting'.
-- Keep existing source migration immutable; replace guard additively.
CREATE OR REPLACE FUNCTION guard_attached_storage_delete()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.state='attached'
       OR EXISTS(
           SELECT 1 FROM chapter_versions
            WHERE storage_object_id=OLD.id
       )
       OR EXISTS(
           SELECT 1 FROM chapter_encrypted_release_events
            WHERE storage_object_id=OLD.id
       ) THEN
        RAISE EXCEPTION 'referenced private object cannot be deleted'
            USING ERRCODE='23514';
    END IF;
    RETURN OLD;
END;
$$;
