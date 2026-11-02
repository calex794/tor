-- ACCESS-AUDIT-01/02: account -> trusted source -> distinct login session.
-- Old session tokens remain valid; existing sessions receive random internal IDs.
-- This audit is separate from hashed auth_attempts and consent-based RIL.
ALTER TABLE sessions
    ADD COLUMN session_id UUID NOT NULL DEFAULT gen_random_uuid();
ALTER TABLE sessions
    ADD CONSTRAINT sessions_internal_id_unique UNIQUE (session_id);

-- Each successful browser authentication inserts exactly one event in the
-- SAME SQL statement/transaction that creates its session. The event stays
-- if an expired session is eventually pruned; account deletion removes it.
CREATE TABLE account_login_events (
    session_id UUID PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    verified_ip INET,
    agent_family TEXT NOT NULL CHECK (
        agent_family IN ('edge', 'chrome', 'firefox', 'safari', 'other', 'unknown')
    ),
    event_kind TEXT NOT NULL CHECK (
        event_kind IN ('registration', 'password_login')
    ),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX account_login_events_by_user_time
    ON account_login_events(user_id, created_at DESC);
CREATE INDEX account_login_events_retention
    ON account_login_events(created_at);

-- Raw IP must have restricted reads and a bounded retention cleanup owner
-- before rollout outside test environments. GeoIP and auto-bans stay OFF.
