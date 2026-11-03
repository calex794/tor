-- S020: independent revision for owner-controlled title edits.
-- Do not change source identity, ownership, visibility, or reader rights.
ALTER TABLE books ADD COLUMN title_metadata_revision BIGINT NOT NULL DEFAULT 1
    CHECK (title_metadata_revision > 0);
