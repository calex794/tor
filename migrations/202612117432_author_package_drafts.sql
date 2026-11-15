-- H/UI-17: a private, owner-scoped selection draft for package composition.
-- A draft is not a paid entitlement, published composition, or offer.
-- The composition version must be rechecked under the Layer lock on stage/apply.
CREATE TABLE commerce_layer_composition_drafts (
    layer_id BIGINT PRIMARY KEY REFERENCES commerce_layers(id) ON DELETE RESTRICT,
    owner_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    expected_version_id BIGINT NOT NULL,
    revision BIGINT NOT NULL CHECK (revision > 0),
    additions BIGINT[] NOT NULL DEFAULT '{}'::BIGINT[],
    removals BIGINT[] NOT NULL DEFAULT '{}'::BIGINT[],
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (layer_id, expected_version_id)
        REFERENCES commerce_layer_versions(layer_id,id) ON DELETE RESTRICT,
    CHECK (cardinality(additions) + cardinality(removals) <= 200)
);
CREATE INDEX commerce_layer_composition_drafts_by_owner
    ON commerce_layer_composition_drafts(owner_user_id,updated_at);
