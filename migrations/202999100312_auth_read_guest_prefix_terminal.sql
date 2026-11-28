-- AUTH-READ-02c1: distinguish terminal unsupported markup from missing work.
-- Existing exact derivatives remain ready. No new retention obligation.
ALTER TABLE chapter_version_guest_prefixes
    ADD COLUMN projection_status TEXT NOT NULL DEFAULT 'ready'
        CHECK (projection_status IN ('ready','unsupported')),
    ADD CONSTRAINT guest_prefix_unsupported_is_empty CHECK (
        projection_status='ready'
        OR (visible_scalar_count=0 AND prefix_text='')
    );
