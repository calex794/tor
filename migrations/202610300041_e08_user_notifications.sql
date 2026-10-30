-- E-08/E-06 on-site notification projection; transport remains the shared I/G outbox.
-- Consumer holds the source event row lock, projects once and acknowledges atomically.
CREATE TABLE user_notifications (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    recipient_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    actor_user_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
    source_event_id UUID NOT NULL UNIQUE REFERENCES outbox_events(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL CHECK (char_length(kind) BETWEEN 1 AND 40 AND kind ~ '^[a-z_]+,
    title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 160),
    body TEXT NOT NULL CHECK (char_length(body) <= 500),
    url TEXT NOT NULL CHECK (char_length(url) BETWEEN 1 AND 1024
                             AND left(url, 1)='/' AND left(url, 2)<>'//'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    read_at TIMESTAMPTZ
);
CREATE INDEX user_notifications_recipient_page
    ON user_notifications(recipient_user_id,id DESC);
CREATE INDEX user_notifications_unread
    ON user_notifications(recipient_user_id,id DESC) WHERE read_at IS NULL;

COMMENT ON TABLE user_notifications IS
    'Shared in-app user notification projection; no external email or push transport.';
),
    title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 160),
    body TEXT NOT NULL CHECK (char_length(body) <= 500),
    url TEXT NOT NULL CHECK (char_length(url) BETWEEN 1 AND 512
                             AND left(url, 1)='/' AND left(url, 2)<>'//'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    read_at TIMESTAMPTZ
);
CREATE INDEX user_notifications_recipient_page
    ON user_notifications(recipient_user_id,id DESC);
CREATE INDEX user_notifications_unread
    ON user_notifications(recipient_user_id,id DESC) WHERE read_at IS NULL;

COMMENT ON TABLE user_notifications IS
    'Shared in-app user notification projection; no external email or push transport.';
