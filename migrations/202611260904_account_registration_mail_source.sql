-- MAIL-01f2b: immutable signup evidence, not linked to a session lifetime.
CREATE TABLE account_registration_mail_sources (
    user_id BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    recipient_key BYTEA NOT NULL CHECK (octet_length(recipient_key) = 32),
    registration_ip_key BYTEA CHECK (octet_length(registration_ip_key) = 32),
    key_binding BYTEA NOT NULL CHECK (octet_length(key_binding) = 32),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX account_registration_mail_sources_expiry
    ON account_registration_mail_sources(created_at, user_id);
