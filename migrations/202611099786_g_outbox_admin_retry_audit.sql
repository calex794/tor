-- G-07 / S066: immutable audit of administrator dead-letter requeues.
-- No production migration rollout or new notification queue in this PR.
CREATE TABLE g_outbox_admin_retry_audit (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    admin_user_id BIGINT NOT NULL CHECK (admin_user_id > 0),
    event_id UUID NOT NULL REFERENCES outbox_events(id) ON DELETE RESTRICT,
    previous_attempts INTEGER NOT NULL CHECK (previous_attempts >= 0),
    previous_lease_epoch BIGINT NOT NULL CHECK (previous_lease_epoch >= 0),
    resumed_lease_epoch BIGINT NOT NULL
        CHECK (resumed_lease_epoch = previous_lease_epoch + 1),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX g_outbox_admin_retry_by_actor
    ON g_outbox_admin_retry_audit (admin_user_id, id DESC);
CREATE INDEX g_outbox_admin_retry_by_event
    ON g_outbox_admin_retry_audit (event_id, id DESC);

CREATE FUNCTION g_outbox_admin_retry_audit_immutable()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'G outbox administrator retry audit is append-only'
        USING ERRCODE = '23514';
END;
$$;
CREATE TRIGGER g_outbox_admin_retry_audit_no_mutation
    BEFORE UPDATE OR DELETE ON g_outbox_admin_retry_audit
    FOR EACH ROW EXECUTE FUNCTION g_outbox_admin_retry_audit_immutable();
