-- E-08/S057: per-kind bell preferences, not content delivery timing.
-- Default enabled is represented by absence; all known checkbox kinds persist on POST.
CREATE TABLE user_notification_preferences (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (char_length(kind) BETWEEN 1 AND 40 AND kind ~ '^[a-z_]+$'),
    enabled BOOLEAN NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (user_id,kind)
);

COMMENT ON TABLE user_notification_preferences IS
    'Actor-owned on-site notification preferences; missing row means enabled; no external send.';
