-- S019: optimistic lock for editing a book's known source links.
-- Existing books retain their source_identity, ownership and publication state.
ALTER TABLE books
    ADD COLUMN registration_links_revision BIGINT NOT NULL DEFAULT 1
    CHECK (registration_links_revision > 0);
