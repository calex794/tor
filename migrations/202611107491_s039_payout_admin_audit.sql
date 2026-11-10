-- S039 / D-09: append-only evidence for privileged payout reservation rejection.
-- A rejection releases a pending internal reservation; it is NOT a wallet
-- debit, a payment posting, or evidence of a bank transfer.
CREATE TABLE finance_creator_payout_admin_audit (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    admin_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    payout_request_id BIGINT REFERENCES finance_creator_payout_requests(id)
        ON DELETE RESTRICT,
    action TEXT NOT NULL CHECK (action IN ('list_requests','reject')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CHECK ((action='list_requests' AND payout_request_id IS NULL)
        OR (action='reject' AND payout_request_id IS NOT NULL))
);
CREATE INDEX s039_payout_audit_admin_recent
    ON finance_creator_payout_admin_audit(admin_user_id,id DESC);
CREATE UNIQUE INDEX s039_one_rejection_audit_per_request
    ON finance_creator_payout_admin_audit(payout_request_id)
    WHERE action='reject';
CREATE TRIGGER s039_payout_admin_audit_immutable
    BEFORE UPDATE OR DELETE ON finance_creator_payout_admin_audit
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();
COMMENT ON TABLE finance_creator_payout_admin_audit IS
    'Immutable admin read/reject decisions. No approval or money movement.';
