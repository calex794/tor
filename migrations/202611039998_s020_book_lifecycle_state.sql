-- S020: NEwBOO lifecycle is distinct from editorial project_status and visibility.
-- pending_deletion is reserved for a separate password/step-up gated flow;
-- the ordinary Book member editor cannot enter or leave that state.
ALTER TABLE books
    ADD COLUMN lifecycle_status TEXT NOT NULL DEFAULT 'in_progress'
        CHECK (lifecycle_status IN (
            'in_progress', 'completed', 'abandoned', 'on_hiatus', 'pending_deletion'
        )),
    ADD COLUMN lifecycle_revision BIGINT NOT NULL DEFAULT 1
        CHECK (lifecycle_revision > 0);
