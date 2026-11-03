-- S020: independent revision for origin language, publication year and catalog section.
-- No changes to publication visibility, ownership, purchases or chapter rights.
ALTER TABLE books
    ADD COLUMN origin_metadata_revision BIGINT NOT NULL DEFAULT 1
        CHECK (origin_metadata_revision > 0);
