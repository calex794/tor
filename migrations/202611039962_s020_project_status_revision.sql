-- S020: owner-controlled editorial progress, distinct from visibility and
-- access_level. Revision is exclusive to this metadata field and never changes
-- reader entitlements or published chapters.
ALTER TABLE books ADD COLUMN project_status_revision BIGINT NOT NULL DEFAULT 1
    CHECK (project_status_revision > 0);
