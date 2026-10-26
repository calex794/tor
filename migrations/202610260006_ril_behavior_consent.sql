-- A / RIL-07: explicit consent only; collection disabled for all existing users.
-- No session tokens or pseudonymous path histories are stored in PostgreSQL.
CREATE TABLE ril_behavior_consents (
    user_id BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    epoch BIGINT NOT NULL DEFAULT 1 CHECK (epoch BETWEEN 1 AND 9223372036854775806),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- Consent is absent (off) for every existing and newly registered user.
-- Each change increments epoch: old HMAC pseudonyms cannot be silently reused.
