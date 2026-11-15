-- H/UI-18: package-specific cover preference for an already approved Book cover.
-- No image upload, moderation bypass or paid entitlement changes.
ALTER TABLE commerce_layers
    ADD COLUMN cover_book_id BIGINT REFERENCES books(id) ON DELETE RESTRICT;
