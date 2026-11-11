-- S039 / MONEY-03 approval readiness: read-only, durable evidence.
-- Neither a terminal payout nor an external/bank transfer.
CREATE TABLE finance_creator_payout_approval_read_audit (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    admin_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    payout_request_id BIGINT NOT NULL REFERENCES finance_creator_payout_requests(id)
        ON DELETE RESTRICT,
    amount_minor BIGINT NOT NULL CHECK (amount_minor > 0),
    other_reserved_minor BIGINT NOT NULL CHECK (other_reserved_minor >= 0),
    available_excluding_own_minor BIGINT NOT NULL
        CHECK (available_excluding_own_minor >= 0),
    sufficient BOOLEAN NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX s039_payout_approval_read_by_request
    ON finance_creator_payout_approval_read_audit(payout_request_id,id DESC);
CREATE TRIGGER s039_payout_approval_read_immutable
    BEFORE UPDATE OR DELETE ON finance_creator_payout_approval_read_audit
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
COMMENT ON TABLE finance_creator_payout_approval_read_audit IS
    'Audited advisory capacity check before trusted approval. No wallet movement.';
