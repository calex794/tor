-- Durable rate limits shared across application instances.
-- Store hashes of rate-limit keys, never plaintext email addresses or IPs.
CREATE TABLE auth_attempts (
    key_hash BYTEA PRIMARY KEY CHECK (octet_length(key_hash) = 32),
    attempts INTEGER NOT NULL CHECK (attempts > 0),
    period_started TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX auth_attempts_started_idx ON auth_attempts (period_started);
