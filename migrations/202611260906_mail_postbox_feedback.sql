-- MAIL-01g2a: retain correlation independently of the 48h quota ledger.
CREATE TABLE mail_feedback_attempts (
    outbox_id BIGINT NOT NULL CHECK (outbox_id>0),
    generation INTEGER NOT NULL CHECK (generation>0),
    recipient_key BYTEA NOT NULL CHECK (octet_length(recipient_key)=32),
    key_binding BYTEA NOT NULL CHECK (octet_length(key_binding)=32),
    identity_id TEXT NOT NULL CHECK (length(identity_id) BETWEEN 1 AND 128),
    marker TEXT NOT NULL UNIQUE CHECK (marker ~ '^[0-9a-f]{64}$'),
    provider_message_id TEXT CHECK (length(provider_message_id) BETWEEN 1 AND 128),
    reserved_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY(outbox_id,generation),
    UNIQUE(identity_id,provider_message_id)
);
CREATE INDEX mail_feedback_attempts_expiry ON mail_feedback_attempts(reserved_at);
CREATE TABLE mail_feedback_events (
    identity_id TEXT NOT NULL CHECK (length(identity_id) BETWEEN 1 AND 128),
    event_id TEXT NOT NULL CHECK (length(event_id) BETWEEN 1 AND 128),
    outbox_id BIGINT NOT NULL,
    generation INTEGER NOT NULL,
    PRIMARY KEY(identity_id,event_id),
    FOREIGN KEY(outbox_id,generation) REFERENCES mail_feedback_attempts(outbox_id,generation)
        ON DELETE CASCADE
);
