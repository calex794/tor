-- S036 / D-06: administrator refund review reads are audited.
-- No money movement, approval, rejection, policy creation or deployment.
-- Never persist the request reason or purchase data in the audit.
CREATE TABLE finance_direct_refund_admin_access_audit (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    admin_user_id BIGINT NOT NULL CHECK (admin_user_id > 0),
    action TEXT NOT NULL CHECK (action = 'list_refunds'),
    before_id BIGINT CHECK (before_id IS NULL OR before_id > 0),
    result_count INTEGER NOT NULL CHECK (result_count BETWEEN 0 AND 30),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX finance_refund_admin_access_actor_recent
    ON finance_direct_refund_admin_access_audit (admin_user_id, id DESC);

-- Audits cannot be silently rewritten or deleted by a runtime DB account.
CREATE FUNCTION d06_refund_admin_access_audit_immutable()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'admin refund review audit is immutable' USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER d06_refund_admin_access_immutable
    BEFORE UPDATE OR DELETE ON finance_direct_refund_admin_access_audit
    FOR EACH ROW EXECUTE FUNCTION d06_refund_admin_access_audit_immutable();
