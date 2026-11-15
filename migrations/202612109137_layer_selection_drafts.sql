-- H/UI-17: private, revocable author scratch state for cross-page package selection.
-- This table stores no purchased rights, pricing, or reader identities.
-- Apply only through normal deployment migration review; never rewrite layer history.
CREATE TABLE commerce_layer_selection_drafts (
    layer_id BIGINT PRIMARY KEY REFERENCES commerce_layers(id) ON DELETE CASCADE,
    owner_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    base_version_id BIGINT NOT NULL,
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision>0),
    added_book_ids BIGINT[] NOT NULL DEFAULT '{}',
    removed_book_ids BIGINT[] NOT NULL DEFAULT '{}',
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (layer_id,base_version_id)
        REFERENCES commerce_layer_versions(layer_id,id) ON DELETE RESTRICT,
    CONSTRAINT layer_draft_bounds CHECK (
        cardinality(added_book_ids)<=100 AND cardinality(removed_book_ids)<=100
    )
);
CREATE INDEX commerce_layer_selection_drafts_owner
    ON commerce_layer_selection_drafts(owner_user_id,updated_at);
