-- S025/G: durable identities for *public* CDN cover copies.
-- This is a fail-closed ledger, NOT permission to enable public delivery.
-- Every reservation must be committed BEFORE copying to the public S3 bucket.
-- The worker may only delete exact identities; purge is a separate cloud operation.
CREATE TABLE public_cover_publications (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    job_id BIGINT NOT NULL,
    slot_revision BIGINT NOT NULL CHECK (slot_revision > 0),
    variant_name TEXT NOT NULL CHECK (variant_name IN ('card', 'main')),
    content_sha256 BYTEA NOT NULL CHECK (octet_length(content_sha256) = 32),
    size_bytes BIGINT NOT NULL CHECK (size_bytes BETWEEN 1 AND 8388608),
    state TEXT NOT NULL DEFAULT 'reserved' CHECK (
        state IN ('reserved', 'copied', 'removal_due', 'origin_deleted',
                  'purge_submitted', 'purged')
    ),
    lease_epoch BIGINT NOT NULL DEFAULT 0 CHECK (lease_epoch >= 0),
    lease_until TIMESTAMPTZ,
    retry_after TIMESTAMPTZ,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    cdn_operation_id TEXT CHECK (
        cdn_operation_id IS NULL OR char_length(cdn_operation_id) BETWEEN 1 AND 128
    ),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (book_id, slot_revision, variant_name),
    FOREIGN KEY (job_id, variant_name)
        REFERENCES cover_variants(job_id, variant_name) ON DELETE RESTRICT,
    CONSTRAINT cover_publication_lease_only_for_removal CHECK (
        lease_until IS NULL OR state = 'removal_due'
    ),
    CONSTRAINT cover_publication_operation_only_after_origin_delete CHECK (
        cdn_operation_id IS NULL OR state IN ('purge_submitted', 'purged')
    )
);
CREATE INDEX public_cover_publications_recovery_idx
    ON public_cover_publications(state, retry_after, lease_until, id)
    WHERE state IN ('reserved', 'copied', 'removal_due', 'origin_deleted', 'purge_submitted');

-- Defense in depth: never associate a different Book's cover job with a
-- public-object record, even if a privileged caller makes a programming error.
CREATE FUNCTION reject_cross_book_public_cover()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM cover_variant_jobs j
        WHERE j.id = NEW.job_id AND j.book_id = NEW.book_id
    ) THEN
        RAISE EXCEPTION 'public cover job belongs to another book'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER public_cover_same_book
    BEFORE INSERT OR UPDATE OF book_id, job_id ON public_cover_publications
    FOR EACH ROW EXECUTE FUNCTION reject_cross_book_public_cover();
