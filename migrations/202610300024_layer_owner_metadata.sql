-- D-05 / S032: editable owner-only display metadata, separate from purchases.
-- Additive migration: do not modify any previously applied SQLx file.
ALTER TABLE commerce_layers
    ADD COLUMN description TEXT NOT NULL DEFAULT ''
        CHECK (char_length(description) <= 2000),
    ADD COLUMN metadata_revision BIGINT NOT NULL DEFAULT 1
        CHECK (metadata_revision > 0);
