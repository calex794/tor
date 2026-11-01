-- A-07: random reset secrets never enter SQL, RIL, URLs or audit events.
CREATE TABLE password_reset_tokens (
    token_hash BYTEA PRIMARY KEY CHECK (octet_length(token_hash)=32),
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    credential_fingerprint BYTEA NOT NULL CHECK (octet_length(credential_fingerprint)=32),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT now()+INTERVAL '30 minutes',
    consumed_at TIMESTAMPTZ,
    CHECK (expires_at > created_at)
);
CREATE INDEX password_reset_tokens_by_user ON password_reset_tokens(user_id);
CREATE INDEX password_reset_tokens_expiry ON password_reset_tokens(expires_at);
