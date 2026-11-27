-- AUTH-05a: bounded account-visible successful security actions.
-- Existing login history remains canonical; no historical events are fabricated.
CREATE TABLE account_security_events (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    event_kind TEXT NOT NULL CHECK (event_kind IN (
        'session_ended','owner_revoke','owner_revoke_others','password_reset'
    )),
    session_id UUID,
    revoked_count BIGINT NOT NULL CHECK (revoked_count >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CHECK (
        (event_kind IN ('session_ended','owner_revoke') AND session_id IS NOT NULL AND revoked_count=1)
        OR (event_kind='owner_revoke_others' AND session_id IS NULL AND revoked_count>0)
        OR (event_kind='password_reset' AND session_id IS NULL)
    )
);
CREATE INDEX account_security_events_by_user
    ON account_security_events(user_id,created_at DESC,id DESC);
CREATE INDEX account_security_events_retention
    ON account_security_events(created_at,id);

-- The runtime may append and perform the existing retention/account cleanup.
-- Rewriting a previously recorded event is not a supported operation.
CREATE FUNCTION account_security_event_no_rewrite() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'account security event cannot be rewritten' USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER account_security_event_no_rewrite
    BEFORE UPDATE ON account_security_events
    FOR EACH ROW EXECUTE FUNCTION account_security_event_no_rewrite();
