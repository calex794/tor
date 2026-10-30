-- G-08 staged on-site notification ID-intents, not proof of send or read.
-- C-16 on-site feed is reader-scoped; external channel opt-in is separate.
-- This migration number is for Draft only. Coordinate SQLx order with I/#235.
CREATE TABLE notification_event_fanout_scans (
    event_id UUID PRIMARY KEY REFERENCES outbox_events(id) ON DELETE RESTRICT,
    cursor_user_id BIGINT NOT NULL DEFAULT 0 CHECK (cursor_user_id >= 0),
    completed_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE notification_event_recipient_intents (
    event_id UUID NOT NULL REFERENCES outbox_events(id) ON DELETE RESTRICT,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    staged_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (event_id, user_id)
);

COMMENT ON TABLE notification_event_recipient_intents IS
    'Durable on-site metadata candidate, not proof of email, push or read.';
