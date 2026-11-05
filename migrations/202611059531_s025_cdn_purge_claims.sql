-- S025: an independently leased, crash-recoverable Yandex CDN purge stage.
-- Keep historical publication migrations immutable. Reuse lease_epoch for
-- sequential S3-delete and CDN-purge stages; only a current lease can ack.
ALTER TABLE public_cover_publications
    ADD COLUMN purge_attempts INTEGER NOT NULL DEFAULT 0
        CHECK (purge_attempts >= 0);
ALTER TABLE public_cover_publications
    DROP CONSTRAINT cover_publication_lease_only_for_removal;
ALTER TABLE public_cover_publications
    ADD CONSTRAINT cover_publication_lease_only_for_work CHECK (
        lease_until IS NULL OR
        state IN ('removal_due', 'origin_deleted', 'purge_submitted')
    );
ALTER TABLE public_cover_publications
    ADD CONSTRAINT cover_publication_purge_operation_required CHECK (
        state <> 'purge_submitted' OR cdn_operation_id IS NOT NULL
    );
