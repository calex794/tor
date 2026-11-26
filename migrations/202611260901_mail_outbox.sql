-- MAIL-01: durable reference-only queue; no external I/O or rollout flag.
CREATE TABLE mail_outbox (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    purpose TEXT NOT NULL CHECK (purpose IN (
        'registration_confirmation', 'support_reply', 'project_news'
    )),
    event_id BIGINT NOT NULL CHECK (event_id > 0),
    recipient_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN (
        'pending', 'leased', 'accepted', 'failed', 'uncertain', 'cancelled'
    )),
    generation INTEGER NOT NULL DEFAULT 0 CHECK (generation >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    lease_until TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    UNIQUE (purpose, event_id, recipient_user_id),
    CHECK ((status = 'leased') = (lease_until IS NOT NULL)),
    CHECK ((status IN ('accepted', 'failed', 'uncertain', 'cancelled')) =
        (finished_at IS NOT NULL)),
    CHECK ((status = 'pending') = (generation = 0))
);

CREATE INDEX mail_outbox_pending ON mail_outbox ((purpose = 'project_news'), id)
    WHERE status = 'pending';
CREATE INDEX mail_outbox_leases ON mail_outbox (lease_until, id)
    WHERE status = 'leased';
