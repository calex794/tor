-- Administrative editorial surprise changes are audited; this is not an ad ledger.
CREATE TABLE surprise_admin_events (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    actor_user_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
    action TEXT NOT NULL CHECK (action IN ('create','set_state','set_policy')),
    content_id BIGINT REFERENCES surprise_content(id) ON DELETE SET NULL,
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    details JSONB NOT NULL DEFAULT '{}'::jsonb
);
CREATE INDEX surprise_admin_events_at ON surprise_admin_events(recorded_at DESC, id DESC);
