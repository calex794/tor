-- A-02/A-05: account state and global roles, independent of book/team/paid rights.
ALTER TABLE users ADD COLUMN account_state TEXT NOT NULL DEFAULT 'active'
    CHECK (account_state IN ('active','suspended','closed'));
CREATE TABLE account_roles (
    user_id BIGINT NOT NULL REFERENCES users(id),
    role TEXT NOT NULL CHECK (role IN ('author','administrator')),
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    PRIMARY KEY(user_id,role)
);
CREATE TABLE account_role_events (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id),
    role TEXT NOT NULL CHECK (role IN ('author','administrator','state')),
    value TEXT NOT NULL,
    changed_by BIGINT NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX account_role_events_by_user ON account_role_events(user_id,id DESC);

-- Suspending then reactivating an account never resurrects an older cookie.
CREATE FUNCTION revoke_sessions_on_account_state() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.account_state <> 'active' THEN
        UPDATE sessions SET revoked_at=now() WHERE user_id=NEW.id AND revoked_at IS NULL;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER account_state_session_revocation AFTER UPDATE OF account_state ON users
    FOR EACH ROW EXECUTE FUNCTION revoke_sessions_on_account_state();
