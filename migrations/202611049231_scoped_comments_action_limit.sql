-- AB-08/09: actor-scoped public comment quota. Independent of auth attempts.
-- Keep exactly one 60-second window per actor and the hashes of admitted
-- idempotency commands. All updates are made under a PostgreSQL row lock.
CREATE TABLE scoped_action_windows (
    scope TEXT NOT NULL CHECK (scope IN ('comments')),
    actor_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    period_started TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    touched_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    admitted INTEGER NOT NULL DEFAULT 0 CHECK (admitted >= 0),
    PRIMARY KEY (scope, actor_id)
);
CREATE INDEX scoped_action_windows_retention_idx
    ON scoped_action_windows (touched_at);

CREATE TABLE scoped_action_command_keys (
    scope TEXT NOT NULL,
    actor_id BIGINT NOT NULL,
    command_digest BYTEA NOT NULL CHECK (octet_length(command_digest) = 32),
    PRIMARY KEY (scope, actor_id, command_digest),
    FOREIGN KEY (scope, actor_id)
        REFERENCES scoped_action_windows(scope, actor_id) ON DELETE CASCADE
);
