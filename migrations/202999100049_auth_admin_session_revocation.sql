-- AUTH-04a: successful administrative session revocations are audited
-- atomically with the session update. No credentials or token hashes.
CREATE TABLE account_admin_session_revocation_events (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    administrator_user_id BIGINT NOT NULL CHECK (administrator_user_id > 0),
    target_user_id BIGINT NOT NULL CHECK (target_user_id > 0),
    selected_session_id UUID,
    revoked_count BIGINT NOT NULL CHECK (revoked_count > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CHECK (administrator_user_id <> target_user_id),
    CHECK (selected_session_id IS NULL OR revoked_count = 1)
);
CREATE INDEX account_admin_session_revocation_by_actor
    ON account_admin_session_revocation_events (administrator_user_id, id DESC);
CREATE INDEX account_admin_session_revocation_by_target
    ON account_admin_session_revocation_events (target_user_id, id DESC);

CREATE FUNCTION account_admin_session_revocation_immutable()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'administrator session revocation audit is append-only'
        USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER account_admin_session_revocation_no_mutation
    BEFORE UPDATE OR DELETE ON account_admin_session_revocation_events
    FOR EACH ROW EXECUTE FUNCTION account_admin_session_revocation_immutable();
