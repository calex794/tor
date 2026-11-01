-- C/D-06: first successfully assembled protected Layer grant evidence.
-- This records issuance of the encrypted-content credential, not a claim
-- that the browser subsequently downloaded or decrypted the ciphertext.
-- Never synthesize "unopened" for historical purchases predating this gate.
CREATE TABLE commerce_layer_term_first_exposures (
    term_id BIGINT PRIMARY KEY REFERENCES commerce_layer_terms(id) ON DELETE RESTRICT,
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL,
    object_kind TEXT NOT NULL CHECK (object_kind IN ('chapter', 'asset')),
    granted_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    FOREIGN KEY (chapter_id,version_id)
        REFERENCES chapter_versions(chapter_id,id) ON DELETE RESTRICT
);

-- The first grant is monotonic. Later grants may not reset the version, kind
-- or timestamp, even if a future admin tries to rewrite historical evidence.
CREATE TRIGGER c06_layer_first_exposures_immutable
    BEFORE UPDATE OR DELETE ON commerce_layer_term_first_exposures
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
