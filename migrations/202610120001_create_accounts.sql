-- User credentials and revocable server-side sessions.
-- Application-facing emails must be normalized before insertion.
CREATE TABLE users (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    email_normalized TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT email_normalized_valid CHECK (
        length(email_normalized) BETWEEN 3 AND 254
        AND email_normalized = lower(btrim(email_normalized))
        AND position('@' in email_normalized) > 1
    ),
    CONSTRAINT display_name_valid CHECK (
        char_length(display_name) BETWEEN 1 AND 80
        AND display_name = btrim(display_name)
    ),
    CONSTRAINT password_hash_is_argon2id CHECK (
        left(password_hash, 10) = '$argon2id$'
    )
);

CREATE TABLE sessions (
    token_hash BYTEA PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    CONSTRAINT session_token_sha256 CHECK (octet_length(token_hash) = 32),
    CONSTRAINT session_has_expiry CHECK (expires_at > created_at)
);

CREATE INDEX sessions_by_user ON sessions (user_id);
CREATE INDEX sessions_active_expiry ON sessions (expires_at) WHERE revoked_at IS NULL;
