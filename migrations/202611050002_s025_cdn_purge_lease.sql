-- S025: allow bounded, fenced leases while submitting and polling Yandex CDN purge.
-- Existing rows are preserved. This does not enable a production worker.
ALTER TABLE public_cover_publications
    DROP CONSTRAINT cover_publication_lease_only_for_removal;
ALTER TABLE public_cover_publications
    ADD CONSTRAINT cover_publication_lease_only_for_active_work CHECK (
        lease_until IS NULL
        OR state IN ('removal_due', 'origin_deleted', 'purge_submitted')
    );
CREATE INDEX public_cover_purge_due_idx
    ON public_cover_publications(state, retry_after, lease_until, id)
    WHERE state IN ('origin_deleted', 'purge_submitted');
