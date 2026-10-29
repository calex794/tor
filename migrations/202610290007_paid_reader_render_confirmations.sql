-- C-06: browser reports a verified render, distinct from credential issuance.
-- Client ACK is not proof of human reading; missing ACK is not proof of non-use.
-- A prior image grant is not proof that the chapter text was ever issued.
-- A second first-grant receipt is created when the chapter itself is signed,
-- even when an earlier *asset* or other chapter already used the term.
CREATE TABLE chapter_purchase_text_grants (
    purchase_id BIGINT PRIMARY KEY
        REFERENCES chapter_purchase_first_exposures(purchase_id) ON DELETE RESTRICT,
    user_id BIGINT NOT NULL,
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL,
    granted_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    FOREIGN KEY (purchase_id,user_id,chapter_id,version_id)
        REFERENCES chapter_purchase_facts(id,user_id,chapter_id,version_id)
        ON DELETE RESTRICT
);
CREATE TRIGGER c06_direct_text_grants_immutable
    BEFORE UPDATE OR DELETE ON chapter_purchase_text_grants
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();

CREATE TABLE commerce_layer_term_text_grants (
    term_id BIGINT NOT NULL
        REFERENCES commerce_layer_term_first_exposures(term_id) ON DELETE RESTRICT,
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL,
    granted_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (term_id,chapter_id,version_id),
    FOREIGN KEY (chapter_id,version_id)
        REFERENCES chapter_versions(chapter_id,id) ON DELETE RESTRICT
);
CREATE TRIGGER c06_layer_text_grants_immutable
    BEFORE UPDATE OR DELETE ON commerce_layer_term_text_grants
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();

CREATE TABLE chapter_purchase_render_confirmations (
    purchase_id BIGINT PRIMARY KEY
        REFERENCES chapter_purchase_text_grants(purchase_id) ON DELETE RESTRICT,
    user_id BIGINT NOT NULL,
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL,
    evidence_kind TEXT NOT NULL DEFAULT 'browser_render_ack_v1'
        CHECK (evidence_kind='browser_render_ack_v1'),
    confirmed_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    FOREIGN KEY (purchase_id,user_id,chapter_id,version_id)
        REFERENCES chapter_purchase_facts(id,user_id,chapter_id,version_id)
        ON DELETE RESTRICT
);
CREATE TRIGGER c06_direct_render_confirmations_immutable
    BEFORE UPDATE OR DELETE ON chapter_purchase_render_confirmations
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();

CREATE TABLE commerce_layer_term_render_confirmations (
    term_id BIGINT PRIMARY KEY,
    chapter_id BIGINT NOT NULL REFERENCES chapters(id) ON DELETE RESTRICT,
    version_id BIGINT NOT NULL REFERENCES chapter_versions(id) ON DELETE RESTRICT,
    FOREIGN KEY (term_id,chapter_id,version_id)
        REFERENCES commerce_layer_term_text_grants(term_id,chapter_id,version_id)
        ON DELETE RESTRICT,
    evidence_kind TEXT NOT NULL DEFAULT 'browser_render_ack_v1'
        CHECK (evidence_kind='browser_render_ack_v1'),
    confirmed_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE TRIGGER c06_layer_render_confirmations_immutable
    BEFORE UPDATE OR DELETE ON commerce_layer_term_render_confirmations
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
